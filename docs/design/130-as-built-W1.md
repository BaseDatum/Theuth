# 130 — as-built W1: the Swift companion core (rail · Attention panel · hotkey · menu bar extra)

Status: built 2026-10-07 on `feat/130-w1-companion` (cut from main `eba311ed`), against TODAY's engine RPC
(2.10.0, no W0 fields). Design: `docs/design/130-companion.md` §0 D1, D2, D4, D5, D6, D9, D11; §4.1, §4.2;
§5.1, §5.2, §5.4, §5.6; §7 row W1; §10 mock round 2 frames 2–5 and 8. Toolchain: Command Line Tools only
(Swift 6.3.2, macOS 26.6.2) — no Xcode, as spike SA proved. Addenda A1 (Revision 1 lifecycle rules), A2 (ad-hoc
signing, debug mode) and A3 (theuth's tray glyph and .icns, not an SF Symbol / drawn icon) applied.

## 1. File map (path:line)

`crates/desktop/companion/` — SwiftPM package `TheuthCompanion` (`Package.swift:1-40`: tools-version 6.0,
`platforms: [.macOS(.v15)]`, Swift 5 language mode on every target, one dependency
`sindresorhus/KeyboardShortcuts` pinned `exact: "1.10.0"` — see §3).

| Path | What |
|---|---|
| `Sources/TheuthKit/Discovery.swift:5-28` | `EngineLocation.discover(dataDir:)`: `<data_dir>/engine.json` → `listen`, else `127.0.0.1:7777`; token `<data_dir>/token`; `THEUTH_DATA_DIR` overrides `~/.theuth` |
| `Sources/TheuthKit/RpcClient.swift:7-193` | `actor RpcClient`: JSON-RPC 2.0 over `URLSessionWebSocketTask`, `Authorization: Bearer` + `X-Theuth-Client: companion` on the upgrade, positional params, numeric ids, one `subscribe_events(null, null)` re-issued on every open (`:163-171`), backoff 1 s → 30 s (`:115-130`), pending calls failed on loss |
| `Sources/TheuthKit/RpcClient.swift:196-235` | The twelve calls: `attention_list`, `attention_snooze`, `attention_dismiss`, `agents_list`, `list_activity`, `packet_get`, `answer_ask`, `resolve_approval`, `task_decide`, `task_move`, `send_input(session, text, [], null, null)`, plus `subscribe_events` — nothing else |
| `Sources/TheuthKit/Models.swift:10-43` | `AttentionKind` with the 12 kinds + `finished` (W0) + `.other(String)` for unknown kinds; `needsYou` = every kind but `show`/`finished`/unknown |
| `Models.swift:45-97` | `AttentionItem` (`attention.rs:68-101`) + optional W0 `waiting_since_ms`, `summary`, `next_steps`; `waitingSince` falls back to `at_ms` |
| `Models.swift:99-137` | `AttentionChanged` (W0), `AgentStatus`, `AgentFile` (name/title/model/paused only), `AgentSummary` (`types.rs:249-284`) |
| `Models.swift:139-240` | `AgentState` (tag `state`, `UI/rpc/extra.ts:80-86`), `SessionActivity` (+ optional W0 `waiting_since_ms`), `AgentInfo` (`agent/src/lib.rs:887-944`; `activity` defaults from `state` on pre-62 engines) |
| `Models.swift:242-333` | `Risk`, `Decision` (`allow`/`deny` only), `ApprovalRequested`/`Resolved` (`approval.rs:30-66`), `Choice`/`Question`/`Answer`/`AskRequested`/`AskResolved` (`ask.rs:35-105`) |
| `Models.swift:336-416` | `Packet` subset (`packet.rs:76-234`: question, summary, board, options, recommended, pointers, checks, recommendation, to); `preferredOption` = `recommendation.option ?? recommended` (103 F2) |
| `Models.swift:418-469` | `PendingPrompts`, `TurnFinished`, `Lagged`, `WireEvent.decode` by `type` (`.other(type)` for the rest) and the 13 `attentionTriggers` of `UI/components/hud/queries.ts:24-43` |
| `Sources/TheuthKit/JSON.swift` | `JSONValue`, the wire codec (`convertFromSnakeCase`, RFC 3339 with/without fractional seconds, sorted keys), `AnyEncodable` positional params |
| `Tests/TheuthKitTests/Fixtures/*.json` (15 files) | Hand-written from the Rust shapes: `attention_list` (approval · decision · ask · review · hire · show), `attention_item_finished` (W0), `attention_item_unknown_kind`, `attention_changed`, `agents_list`, `list_activity`, `approval_requested/resolved`, `ask_requested/resolved`, `turn_finished`, `lagged`, `pending_prompts`, `packet_decision`, `packet_review` |
| `Tests/TheuthKitTests/FixtureTests.swift` | The contract test: every fixture decodes; unknown kind → `.other`; `Answer`/`Decision` encode exactly as serde (`{"index":1,"kind":"choice"}`, `["id-1",null]`); discovery |
| `Sources/CompanionCore/RailModel.swift:51-105` | `RailModel.derive(items:agents:activity:now:) -> Rail`: zone 1 needs-you oldest first (`waiting_since_ms ?? at_ms`), zone 2 working · idle · finished · error · lapsed; desks from `agents_list`, root sessions with a live actor from `list_activity` (a desk's own session is not doubled; `proposed` is already a `hire:` item); ≤ 12 marks then `overflow`; `quiet` = no needs-you and nothing working; `ago()` formatting |
| `Sources/CompanionCore/PanelModel.swift` | Order (needs-you oldest first → finished → receipts), focus index, `move(±1)` clamped, `update(items:)` keeps the focused id, headline "3 need you · 5 working · 2 done" / "Nothing needs you", position "1 of 3" |
| `Sources/CompanionCore/UndoQueue.swift` | `UndoQueue<Action>`: `hold` (a second hold displaces and returns the first), `cancel`, `tick(now:)` fires once, `remaining`; delay clamped 0–5 s, default 3; pure with injected `now` |
| `Sources/CompanionCore/KeyGrammar.swift:32-62` | `KeyGrammar.intent(for: KeyPress) -> Intent`: 1/2/3 · ⏎ · ⎋ · J/K · ↓/↑ · S · E · D · O · ⌘O; `DeepLink` (`theuth://attention/<id>`, `theuth://c/<session>`, `theuth://new`); `StaleGuard.allows(id:latest:)` |
| `Sources/CompanionCore/CardModel.swift` | `Card.make(item:packet:approval:agentModel:)` → who · question · why · ≤ 3 `CardOption`s with key badges and the pre-pick, chips, `moreOptions`; one `CardAction` per option; P13: destructive approvals and failing-check reviews have no pre-pick |
| `Tests/CompanionCoreTests/CompanionCoreTests.swift` | 16 tests: zone ordering incl. E2 precedence, cap/overflow, quiet, panel order/headline/focus, undo timing (2.999 s no · 3 s yes · once · cancel · displacement · clamp · 0 s), the key table, deep links, every card kind |
| `Sources/Companion/main.swift` | Accessory policy; Revision 1: a second instance (same bundle id) exits 0 at once |
| `Sources/Companion/AppDelegate.swift:7-10` | `KeyboardShortcuts.Name.openPanel`, default `⌃⇧Space` |
| `AppDelegate.swift:21-57` | Launch: status item, rail, panel, hotkey → `toggle(anchor: .mouse)`, engine discovery → `Store.start`; `applicationWillTerminate` → `exit(0)` (Revision 1) |
| `AppDelegate.swift:59-104` | `NSStatusItem`: theuth's own tray glyph (`Contents/Resources/tray{,@2x}.png` from `crates/desktop/icons/`, the Tauri tray's file, template, 18 pt; SF Symbol `circle.hexagongrid` only if the resource is missing, logged — A3); `" ●N"` in gold as `attributedTitle` (a separate, non-template layer) when needs-you > 0; left click → panel under `button.window.frame`; right/⌥-click → menu (Open theuth → `theuth://new`, Quit) |
| `AppDelegate.swift:106-114` | Mark click: needs-you/finished → panel beside the rail on that item; agent → `theuth://c/<session>` |
| `AppDelegate.swift:120-145` | `--probe N`: log counts + first `PendingPrompts`, show rail + panel (not key), quit after N s |
| `Sources/Companion/Store.swift` | `@MainActor Store`: items/agents/activity/approvals/packets; events → 250 ms debounced refetch (`:66-95`), `Lagged`/`PendingPrompts` → immediate; `packet_get` for every decision/review (`:127-132`); `perform(Intent)` (`:148-176`); the hold (`:194-219`) and `commit` with the E5 re-read (`:221-255`): engine `false` → "already answered elsewhere", never retried |
| `Sources/Companion/RailPanel.swift:10-107` | `RailController`: `NSPanel` `[.borderless, .nonactivatingPanel]`, level `.statusBar` set once (never `isFloatingPanel`), `canJoinAllSpaces | fullScreenAuxiliary | stationary`, `sharingType = .none` unless `--shareable`, `ignoresMouseEvents` flipped by a global mouse-moved monitor while the pointer is over the rail, 26 pt at the right (or left) edge of `NSScreen.screens.first`, vertically centred, 4 pt sliver after 8 s quiet, hover reveals |
| `RailPanel.swift:109-213` | `RailView`/`MarkView`: wordmark + gold count, needs-you rings with the kind's notch (12 · 3 · 6 · 9 o'clock), hairline, agents (filled faience 380 ms beat — static under Reduce Motion; hollow; malachite check; carnelian bar; dashed sand), `+N`; tooltip + VoiceOver label "<agent>, <state>, <since>" |
| `Sources/Companion/AttentionPanel.swift` | `KeyPanel` (`canBecomeKey`), `PanelController`: non-activating, `becomesKeyOnlyIfNeeded = false`, 372 pt, ≤ 560 pt, `NSGlassEffectView` behind `#available(macOS 26, *)` else `NSVisualEffectView .hudWindow`, anchors (status item · rail mark · mouse screen 20 % down), local key monitor → `KeyGrammar` (`:140-152`), `place()` sizes to the SwiftUI content before ordering front (`:154-166`), closes on resign-key or ⎋ and re-activates the previously frontmost app (`:129-137`) |
| `Sources/Companion/PanelView.swift` | SwiftUI: header sentence (gold count), `CardView` (who · how long → question → why → options with key badges, "recommended · ⏎", chips, send-back note field), lines (≤ 6 then "+N more · J to walk them"), footer hints, `ReceiptView` with the countdown bar |
| `Sources/Companion/Palette.swift` | Dark/light pairs with the exact `UI/styles.css` values: gold `#c47a2c`/`#b4531e`, faience `#5b8f8a`/`#3d6b66`, malachite `#4a8a6e`/`#2f5d4a`, carnelian `#c45a48`/`#9b2d1f`, bone/soot text and surfaces; gold used only by the wordmark, the count and needs-you marks; `Log`, `Flags` (`--probe`, `--shareable`), `Settings` (`UserDefaults` `undoDelay` 0–5, `railSide`) |
| `crates/desktop/companion/test.sh` | `swift test` with the CLT's Swift Testing framework and interop dylib on the search path (§3) |
| `scripts/bundle-companion.sh` | `swift build -c release` (through `build-lock` when present) → `target/release/companion/Theuth Companion.app`: `CFBundleIdentifier io.theuth.mac.companion`, `LSUIElement`, `LSMinimumSystemVersion 15.0`, `NSPrincipalClass NSApplication`, version from the workspace `Cargo.toml`, `AppIcon.icns` = `crates/desktop/icons/icon.icns` (the main app's, A3) and `tray{,@2x}.png` into `Contents/Resources`, `PkgInfo`, dependency resource bundles into `Contents/Resources`, ad-hoc `codesign --deep` or `THEUTH_SIGN_IDENTITY` with `--options runtime --timestamp`, `codesign --verify --deep --strict` |
| `justfile` (tail) | `just companion` → the script |

## 2. What works live

Gates (130 §6.3, W1): `swift test` green — 24 tests in 6 suites (8 contract-fixture tests in `TheuthKitTests`,
16 in `CompanionCoreTests`), run as `BUILD_LABEL=130-w1 build-lock crates/desktop/companion/test.sh`.
`scripts/bundle-companion.sh` → `codesign --verify --deep --strict --verbose=2`: "valid on disk · satisfies its
Designated Requirement" (ad-hoc). Release build of the three targets: 54 s cold on the loaded machine (load ~30), 4–7 s warm.

Live probe against the user's running engine (`~/.theuth/engine.json` → `127.0.0.1:49431`, engine 2.10.0, not
started or stopped by this wave), `open -g 'target/release/companion/Theuth Companion.app' --args --probe 15
--shareable`, log `/tmp/130-w1-probe.log`:

```
13:36:50.242  launch bundle=io.theuth.mac.companion probe=15.0 shareable=true
13:36:50.424  engine ws://127.0.0.1:49431/ (token 64 chars)
13:36:50.565  connection: open
13:36:50.575  probe PendingPrompts approvals=0 asks=0 secrets=0 first=none
13:36:52.388  probe attention_list=28 [show×4, review×19, decision×6 … blocked] agents_list=6 list_activity=147 live=30
              rail: needsYou=24 working=8 marks=12 quiet=false headline="24 need you · 8 working"
13:37:06.186  probe windows (CGWindowList): ["x=1094 y=312 w=372 h=435", "x=2534 y=690 w=26 h=341"]
13:37:06.187  probe done after 15.0 s — quitting (rail=(2534, 409, 26, 341) panel=(1094, 693, 372, 435)
              frontmost=theuth, active=false, key=none, panel key=false)
13:37:06.192  terminate
```

Counts: `attention_list` 28 items (4 `show`, 19 `review`, 4 `decision`, 1 `blocked`; 24 needs-you), `agents_list` 6
desks, `list_activity` 147 sessions of which 30 live; the first `PendingPrompts` was empty (no approval, ask or
secret pending at 13:36). The rail derived 24 needs-you + 8 working, capped at 12 marks (`+48` tail — with 24
needs-you items every visible mark is gold and zone 2 is entirely in the tail; the rule is the design's ≤ 12).
The connection opened in 323 ms from launch; a full refetch (three calls + `packet_get` for every decision and
review key) landed 1.8 s after the socket opened.

Screenshot `/tmp/130-w1-shot.png` at second 8 (`screencapture -x`, panels `.readOnly` under `--shareable`):
the rail is at the right edge of the main display (x 2534–2560,
y 690–1031 top-down: wordmark, gold `24`, twelve gold rings with their kind notches, the hairline, `+48`); the
Attention panel sits top-centre of the mouse's screen (x 1094–1466, y 312–747), glass over the theuth window, with
the header "● 24 need you · 8 working · 1 of 28", the oldest item's card (`blocked`, nikolai · anthropic/claude-opus-5-5
· waiting 21 h · the task title · "on person:Sky" · `O Open in theuth` · S snooze 30 m · ⌘O session), six lines
(stas · ivan ×4 · dmitri, 21 h), "+21 more · J to walk them", the footer hints and "answers wait 3 s · ⎋ undoes";
the menu bar shows theuth's tray glyph (the same file the Tauri tray item beside it uses) with `●24` in gold (`/tmp/130-w1-crop-menubar.png`, 13:49 run after A3). Verdict: **both surfaces and the status item appeared,
in the right places and sizes; the CGWindowList bounds match the AppKit frames.** (`/tmp/130-w1-shot.png`;
sips crops `/tmp/130-w1-crop-rail.png`, downscale `/tmp/130-w1-shot-small.png`.)

Focus: `osascript` (System Events) read the frontmost process as `ghostty` just before `open -g` and again at
second 8; the companion's own log at quit read `active=false, key=none, panel key=false` (the user had moved to
theuth by then — their switch, not ours). No dialog, no permission prompt, nothing answered: the probe path never
calls a write RPC (`AppDelegate.swift:120-145` only reads) and the probe panel has `ignoresMouseEvents = true`
so a stray click cannot make it key (`AttentionPanel.swift:117-127`). The app quit itself after 15 s and
`pgrep -x Companion` was empty afterwards; exit 0. One earlier run (13:35) ended with `panel key=true,
active=true` while a click-able probe panel was on screen for 15 s — the mouse-ignoring probe panel is the fix,
and the subsequent run above is clean.

Not exercised live (by the brief): the keyboard path — answers, undo, snooze, clear — is covered by unit tests
only (`UndoQueueTests`, `KeyGrammarTests`, `CardModelTests`, `PanelModelTests`); the hotkey registration ran
(no error) but was not pressed; the glass backdrop renders (26.6.2) but was not inspected under Reduce
Transparency.

### 2.1 Signing on this machine (addendum A2)

This Mac has no code-signing identity (spike SC): every local build is ad-hoc (`codesign --force --deep --sign -`).
Observed here: the ad-hoc bundle **did** launch through LaunchServices — `open -g 'target/release/companion/Theuth
Companion.app' --args --probe 15 --shareable` ran five times (12:51, 12:53, 12:56, 13:07, 13:35, 13:36) with no
Gatekeeper refusal and no dialog (a locally built, never-quarantined bundle; `xattr -crs` runs before signing). No
interactive work-around was needed or attempted. Fallbacks that were also exercised and keep the bundle context
(`Bundle.main` resolves the `.app`, so the bundle id and `UNUserNotificationCenter` still work):
`"target/release/companion/Theuth Companion.app/Contents/MacOS/Companion" --probe 5 --shareable` (12:50, exit 0,
same log lines) and the bare debug/release executable `crates/desktop/companion/.build/release/Companion --probe 5
--shareable` (13:14–13:30; bundle id `nil`, fine for the probe, not for notifications — W2 must test from the
bundle). Release signing is the release Mac's job: `scripts/bundle-companion.sh` honours `THEUTH_SIGN_IDENTITY`
(default `-`) and adds `--options runtime --timestamp` only when the identity is not `-`; the nested signing line,
`just package-macos` + `just notarize-macos` and the App ID / provisioning steps are W1S's
(`docs/design/130/spike-SC-entitlements-release.md` §Manual).

## 3. Deviations from the design and the brief

- **No XCTest, no `swift test` without flags.** The Command Line Tools ship Swift Testing
  (`/Library/Developer/CommandLineTools/Library/Developer/Frameworks/Testing.framework`) but no XCTest, and SwiftPM
  does not put that framework or `lib_TestingInterop.dylib` on the search/rpath by itself (`swift test` fails with
  "no such module 'Testing'", then `dlopen … lib_TestingInterop.dylib`). Tests are written with Swift Testing
  (`@Test`, `#expect`, `#require`) and run through `crates/desktop/companion/test.sh`, which adds
  `-F <CLT>/Library/Developer/Frameworks` and the two rpaths. With Xcode installed (W4) plain `swift test` works.
- **KeyboardShortcuts pinned to 1.10.0, not 2.x.** Every 2.x tag carries `#Preview` blocks in `Recorder.swift`;
  the CLT has no `PreviewsMacros` plugin, so the dependency fails to compile outside Xcode. 1.10.0 is the last
  tag without previews and has the same `Name`/`onKeyUp` API. Re-pin to 2.x when Xcode lands (A1).
- **"Allow for this session" is not offered** (design §4.2 approval row): the engine's `Decision` is `Allow | Deny`
  only (`crates/tools/src/approval.rs:30-35`) and `set_approvals` is outside this wave's RPC list. The card shows
  `1 Allow` (pre-picked unless `risk = destructive`, P13) · `2 Deny`.
- **Send-back note is an inline field, not a sheet.** A sheet on a non-activating borderless panel would need its
  own key handling; the required note is a `TextField` that replaces the options (`PanelView.swift:179-196`), ⏎
  sends (then the 3 s hold), ⎋ cancels, empty is refused.
- **Multi-question asks and decision bundles open in theuth.** A multi-question `AskRequested` is not answered one
  question per card; the card says "N questions — answer them in theuth" and `O` opens
  `theuth://attention/<id>`. `Packet.items` (bundles, 103 F3) is not decoded; a bundle shows its packet options
  like any decision.
- **Agent peek card not built** (§4.1 "click an agent mark → a peek card"): clicking an agent mark opens
  `theuth://c/<session>`; the hover tooltip carries name · state · running-for. Cost today (§9 Q9) needs
  `activity_list`, which is outside the W1 RPC list.
- **Deep links are inert until W1S** registers `theuth://` in the Tauri shell: `NSWorkspace.shared.open` is
  called and nothing happens; no fallback, by the brief.
- **R (reply in words) and ⌘L (Ask bar) are not in the grammar** — W3. ⇥ expand is not implemented.
- **Status-item anchoring** is manual placement under `button.window.frame` on every macOS; the SDK on this
  machine (26.5) exposes no `expandedInterfaceDelegate`, so there is no macOS 27 branch.
- **Lines are capped at 6** below the focused card with a "+N more · J to walk them" row so the panel stays
  ≤ 560 pt; J/K move through the whole list.
- **Rail hit-testing** uses a global `mouseMoved` monitor that flips `ignoresMouseEvents` while the pointer is
  within the rail's frame (whole strip, not per mark); global mouse monitors need no permission.
- **`sharingType = .none` hides the rail and panel from `screencapture`** (spike SA): `--shareable` exists only
  for self-checks like this probe.
- **"Nothing needs you" headline** is produced when neither needs-you nor working counts are non-zero; the mock's
  "Two desks idle, next routine 14:30" needs `list_schedules`, outside this wave.

## 4. Open items

- **W2 (notifications, policy, settings):** `UNUserNotificationCenter` categories/actions, `NotificationPolicy`
  + breakpoints + flood + pause, the Settings window (rail side/display, hotkey recorder, undo delay, launch at
  login via `SMAppService`); the "Pause notifications ▸" and "Launch at login" menu items; `Settings.undoDelay`
  and `Settings.railSide` are the `UserDefaults` keys already read.
- **W3 (Ask bar):** the hotkey opening as the Ask bar when nothing needs you (today it opens the panel with
  "Nothing needs you."), `R` reply with the hold, ⌘L, voice via `AVAudioEngine` → `transcribe`, receipts read
  back from `activity_list` (`actor = user@companion`, E4), the hub connection.
- **W1S / integration:** `theuth://` scheme in the Tauri shell (the companion already opens
  `theuth://attention/<id>`, `theuth://c/<session>`, `theuth://new`); nesting `target/release/companion/Theuth
  Companion.app` under `Contents/Library/LoginItems/` and the one inside-out `codesign` line in
  `scripts/package-macos.sh` (spike SA §3); `companion-alive`, tray hand-over, quit handshake. The companion already
  exits 0 on duplicate launch and on quit (Revision 1).
- **W0 follow-through:** when E1–E3 land the companion needs no code change: `AttentionChanged` already
  triggers a refetch, `waiting_since_ms` is preferred over `at_ms`, `finished` items render with summary + next
  steps. Dropping the 13-kind client-side trigger list in favour of `AttentionChanged` alone is a one-line change
  in `Store.handle` once every engine emits it.
- **Scripted day / C1–C13** (130 §6.1) were not run in this wave — no scratch engine with synthetic events; the
  live probe plus unit tests stand in. C10 (never steals key until summoned) was observed on the probe path only.
- Known rough edges: the agent zone's `error` state comes from `AgentState.error` and desks
  `unreachable`/`invalid` only — `budget:<agent>` paused is a needs-you item, not an error mark; the per-mark
  frame used to anchor the panel beside the rail is computed from mark order (16 pt rows), not measured.
