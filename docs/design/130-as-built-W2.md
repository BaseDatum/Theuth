# 130 — as-built W2: notifications by kind · breakpoint policy · Settings · launch at login

Status: built 2026-10-07 on `feat/130-w2-notify` (cut from `integrate/130` at `54962df8`; W0 + W1 + W1S merged).
Design: `docs/design/130-companion.md` D8, D12, §4.4 (policy table), §4.5 (menu), §4.6, §5.5, §5.7; Revision 1
R1-2 (login item registered by the MAIN app), R1-3 (breakpoint signals, double-tap ⌥ behind Accessibility), R1-4
(categories on every launch, no time-sensitive level without the profile). Spikes: SA §5, SB §1/§4/§5, SC §3.
Toolchain as W1: Command Line Tools only, Swift 6.3.2, macOS 26.6.2; Rust 1.98.1 for the one Rust file.

## 1. File map (path:line)

`crates/desktop/companion/` unless noted. Everything new is under the paths this wave owns; shared files got
one-line additive edits (§1.3).

### 1.1 Pure (CompanionCore, tested)

| Path | What |
|---|---|
| `Sources/CompanionCore/Policy.swift:7-35` | `NotificationClass` (blocking · deferrable · error · never), `NotifyMode` (immediate · atBreak · off), `NotificationPrefs` (per class + `soundOnErrors` + `timeSensitiveAvailable = false`, R1-4) |
| `Policy.swift:37-68` | `Banner` (id · category · title · body · thread = agent · level · sound · itemIds; `floodId`, `engineId`), `PolicyOutput { post, withdraw }` |
| `Policy.swift:102-126` | `NotificationPolicy.classify` — §4.4 rows: approval/ask/decision/secret/handoff → blocking; review/hire/finished/blocked → deferrable; `budget` → error when `detail` starts "paused" (`attention.rs:548-551`), else deferrable (80 %); `node` → error; show/resume/unknown → never. `category(for:)`: ask/decision → `question`, approval, review, finished, node/budget-paused → `error`, rest → `batch` |
| `Policy.swift:129-179` | `update(items:now:)` — the diff: withdraw banners whose items are all gone, drop held items that vanished, classify every unseen id; **first call seeds** (items waiting before launch are not "new") and only takes the flood reading; paused → tracked, nothing posted; flood entry/exit (> 10 needs-you → one summary, held cleared; ≤ 10 → summary withdrawn); per class × mode → post now / hold / skip; errors post even in flood |
| `Policy.swift:181-224` | `breakpoint(now:)` (refreshes the flood count, releases the batch), `tick(now:)` (2-minute cap, rate window, end of pause), `pause(until:)`, `engine(connected:now:)` (one error banner on loss, withdrawn on reconnect) |
| `Policy.swift:226-253` | `release`: held → one banner when `breakpointPending || held ≥ 120 s`, **but** not within 120 s of the last deferrable banner (§5.5 "hold even at a breakpoint"); a single held item keeps its own category, several → `batch` "theuth at your break · N for later · M finished", `.passive` |
| `Sources/CompanionCore/Breakpoint.swift:7-43` | `BreakpointDetector.feed(.appActivated(id) / .idle(seconds)) -> Bool`: an app switch (first observation seeds; own bundle id ignored; same app again ignored), idle ≥ 20 s fires once per stretch |
| `Breakpoint.swift:45-96` | `Modifiers` (shift · control · option · command · function) and spike SB's `DoubleTapDetector` ported verbatim onto it (300 ms hold, 300 ms window, auto-repeat ignored, must start from nothing held, `cancel()`) |
| `Sources/CompanionCore/PolicySettings.swift` | `CompanionSettings` — every Settings value, one `UserDefaults` key each (W1's `undoDelay`/`railSide` keys kept), `load(from:)`/`save(to:)` with the 0–5 s clamp, `loginItemJSON` (`{ "launch_at_login": bool }`) |
| `Tests/CompanionCoreTests/PolicyTests.swift` | 26 tests in 6 suites: every class row, blocking `.active` silent, errors `.active`+sound / `.timeSensitive` only when available / sound off, first snapshot seeds; held → one batch at a breakpoint, single item keeps its category, 2-min cap without a breakpoint, steady-state 1 per 2 min with a pending breakpoint, per-class modes; flood (summary, no per-item, refresh at a breakpoint, errors pass, drain withdraws), answered-elsewhere withdrawal (+ batch withdrawn when empty), pause (nothing posted, ends on tick, items seen during pause stay seen), engine lost/back; breakpoint detector (switch/self/same, idle once per stretch); the 10 SB double-tap cases; settings round trip + defaults + clamp + W1 keys + JSON |

### 1.2 AppKit / SwiftUI (Companion, thin)

| Path | What |
|---|---|
| `Sources/Companion/Notifications/Notifier.swift:13-75` | `Notifier: UNUserNotificationCenterDelegate`. `center` is nil without a bundle id (the bare executable crashes in `current()`, SA §5). `start()`: delegate, `registerCategories()` **every launch**, read authorization (no prompt), `store.$items.dropFirst()` → `policy.update` (the first real fetch seeds), `store.$connection` → `policy.engine` (`.reconnecting`/`.closed` = lost, `.open` = back), `store.$receipt` → receipt banner, live prefs from `SettingsModel`, `BreakpointSource` → `breakpoint`/`tick` |
| `Notifier.swift:77-108` | `pause(until:)` (menu), `tomorrow`, `requestAuthorization()` (`[.alert, .sound]`; the ONLY caller sites are the Settings button and `--request-notifications`), `refreshAuthorization()` |
| `Notifier.swift:110-144` | Categories: `question` (opt-1/2/3 + `reply` text "Reply…"), `approval` (Allow once · Deny (destructive) + `deny-note` text), `review` (Approve + `send-back` text + Open), `finished` (Next step 1/2 + Clear), `batch` (Open panel · Later), `error` (Open). Per-banner variants `<category>:<bannerId>` carry W1's real `Card.options` labels (≤ 3) and are merged into the registered set; removed on withdraw |
| `Notifier.swift:154-194` | `post`: `UNMutableNotificationContent` title/body/thread/category/`userInfo.items`/sound/`interruptionLevel` (`.active`/`.passive`/`.timeSensitive`), request id = banner id, `add` errors logged; `dryRun` logs "would post" instead. `withdraw`: `removeDeliveredNotifications` + `removePendingNotificationRequests` |
| `Notifier.swift:208-274` | Taps: body/`open` → `openOnItem` (W1's panel on that item); `opt-N` → the card's N-th non-note option → `Store.answerNow` (**no hold**); `reply` → `answerAsk(.text)`; `deny-note` → deny + note as `send_input`; `send-back` → `reviewSendBackNote`; `clear` → dismiss; `later`/dismiss → nothing. The Store's outcome (`sent`/`stale`/`failed`) becomes a `.passive` receipt banner (no category) |
| `Sources/Companion/Notifications/NotificationsInstaller.swift:8-45` | The one hook: builds `Notifier` (dry-run under `--probe-notify`), `SettingsWindowController`, `DoubleTapSource`; installs the double-tap monitor silently only if already opted in AND trusted (never prompts at launch); writes `companion.json` once if missing; `--request-notifications`; `--probe-notify` |
| `NotificationsInstaller.swift:47-80` | Menu (§4.5): `Pause notifications ▸ 1 hour · Until tomorrow · Off` (check marks), `Launch at login` (check), `Settings… ⌘,` — inserted before W1's trailing "— · Quit" pair |
| `NotificationsInstaller.swift:88-132` | `--probe-notify N` (§3 below) |
| `Sources/Companion/Breakpoints/BreakpointSource.swift:10-60` | `BreakpointSource`: `NSWorkspace.shared.notificationCenter` `didActivateApplicationNotification` → `.appActivated(bundleId)`; a `Timer(1 s, repeats)` added to `RunLoop.main` in `.common` (R1-3, never `asyncAfter`) sampling `idleSeconds()` = IOKit `IOHIDSystem.HIDIdleTime` (ns → s), fallback `CGEventSource.secondsSinceLastEventType(.combinedSessionState, kCGAnyInputEventType)`; the same tick drives the policy's cap |
| `BreakpointSource.swift:62-106` | `DoubleTapSource`: `enable(prompt:)` → `AXIsProcessTrustedWithOptions` (the one allowed Accessibility prompt, from Settings), global + local `flagsChanged` monitors → `DoubleTapDetector` (caps lock masked); keyDown/mouseDown monitor → `cancel()`; `uninstall()` |
| `Sources/Companion/Settings/SettingsStore.swift:9-22` | `Settings.defaults` = the `io.theuth.mac.companion` suite (from the bundle that IS `UserDefaults.standard`, so W1's readers keep working), `collapseWhenQuiet`, `loginItemFile` = `<data_dir>/companion.json` |
| `SettingsStore.swift:25-51` | `SettingsModel.shared` (`@Published value: CompanionSettings`, saved on every change; `launchAtLogin` change → `writeLoginItem()`) |
| `SettingsStore.swift:54-62` | `Flags.requestNotifications`, `Flags.probeNotify`, `Flags.anyProbe` |
| `Sources/Companion/Settings/SettingsWindow.swift:11-46` | `SettingsWindowController`: one `NSWindow` (titled · closable, 500 × 400, `isReleasedWhenClosed = false`, `sharingType` per `--shareable`); `show(activate: true)` from the menu activates and takes key; `show(activate: false)` (probe) `orderFrontRegardless` only |
| `SettingsWindow.swift:48-170` | `SettingsView` tabs — Rail: side (segmented), display (`NSScreen.localizedName`), collapse-when-quiet. Keys: `KeyboardShortcuts.Recorder("Open the panel", name: .openPanel)`, "Double-tap ⌥ as well" (on → `enable(prompt: true)`; not yet trusted → toggle stays off with the System Settings hint), undo delay slider 0–5 s. Notifications: three class pickers (Immediately · At a break · Off), Sound on errors, **Enable notifications** + status ("not asked yet" · on · off in System Settings · quiet), the first-run explanation while `.notDetermined`, pause status, **Launch at login** + the companion.json note. Engine: engine URL, connection, data dir, client id, hub (W3), counts. The probe opens on Notifications |
| `crates/desktop/src/companion/login_item.rs` (Rust, NEW) | `SMAppService.loginItem(identifier: "io.theuth.mac.companion")` via `objc2::msg_send!` + `#[link(name = "ServiceManagement", kind = "framework")]` (no `objc2-service-management` in the vendored registry); `LoginItemStatus` (`not_registered` · `enabled` · `requires_approval` · `not_found` · `unavailable`); `wanted()` reads `<data_dir>/companion.json`; `sync()` registers when wanted (`kSMErrorAlreadyRegistered` = 11 → success) / unregisters otherwise, logs `requires_approval`, returns the status; `status()` live |
| `crates/desktop/src/companion/mod.rs:15-16, 34-49` | `pub mod login_item` (macOS), `#[tauri::command] companion_login_item_status() -> String` (`"unavailable"` elsewhere) |
| `crates/desktop/src/companion/macos.rs:14, 39-40` | `init`: `else if login_item::sync() == Enabled { "launchd starts it" }` before W1S's `locate()`/`launch()` — no double launch while registered |
| `crates/desktop/src/lib.rs:309` | `companion::companion_login_item_status` appended to `generate_handler!` |

### 1.3 Shared files touched (one-line additive, W3 merges around them)

- `AppDelegate.swift:45` `NotificationsInstaller.install(into: self)` at the end of `applicationDidFinishLaunching`;
  `:116` `NotificationsInstaller.shared?.appendMenu(menu)` after W1's `forEach` line; `:167-176` two hooks at the end of
  the class (`openPanel(on:)` — focus + open under the status item, inert under any probe; `togglePanel()`).
- `Store.swift:267-275` `answerNow(_:note:session:)`: `commit(h)` (W1's E5 re-read + RPC) with no hold, optional note →
  `send_input`.
- `RailPanel.swift:55` `&& Settings.collapseWhenQuiet` in the quiet-collapse guard.
- Not touched: `AttentionPanel.swift`, `PanelView.swift`, `main.swift`, `scripts/bundle-companion.sh`,
  `companion.entitlements.plist` (still the empty dict — no time-sensitive entitlement until the profile exists, SC §3).

## 2. What was verified and how

**Gates.** `BUILD_LABEL=130-w2 build-lock crates/desktop/companion/test.sh`: **50 tests in 12 suites passed** (24 W1 +
26 new). `scripts/bundle-companion.sh`: release build, ad-hoc sign, `codesign --verify --deep --strict` "valid on disk ·
satisfies its Designated Requirement". Rust (the one file): `cargo fmt`, `build-lock cargo check -p desktop` (1 m 00 s,
warm cache), `cargo clippy -p desktop -- -D warnings` clean, under `CARGO_TARGET_DIR=$PWD/target CARGO_BUILD_JOBS=4
CARGO_INCREMENTAL=0` and the 1.98.1 toolchain. `register()`/`unregister()` were **not** called in any automated run.

**Live probe** (`open -g 'target/release/companion/Theuth Companion.app' --args --probe-notify 20 --shareable`, four
runs 13:21–13:26 against the user's engine `127.0.0.1:49431`, log `/tmp/130-w2-probe.log`, final run):

```
13:26:33 start probe-notify 20.0 s bundle=io.theuth.mac.companion prefs=…(blocking: immediate, deferrable: atBreak, error: immediate,
         soundOnErrors: true, timeSensitiveAvailable: false) authorization=not requested
13:26:51 settings window shown frame=(1030.0, 733.0, 500.0, 432.0) number=1098 active=false key=none
13:26:54 live items=24 [blocked×1 decision×6 review×17] classes=[blocking×6 deferrable×18] new-while-running→would-have-posted=[:]
         flood=true held=0 paused=false connection=open
13:26:54 replay all 24 at once: post=[batch:flood active]
13:26:54 replay item 1…9 review (deferrable): post=[]                               ← held
13:26:54 replay item 10 decision (blocking): post=[question:decision:… active]       ← immediate, no sound
13:26:54 replay tick +60 s (cap not elapsed, held=9): post=[]
13:26:54 replay breakpoint +61 s (held=9): post=[batch:batch-… passive]              ← one batch banner
13:26:54 replay items re-added +70 s: post=[question:decision:… active]
13:26:54 replay breakpoint +80 s inside the 2-min rate window (held=8): post=[]      ← rate cap holds
13:26:54 replay tick +181 s (rate window passed): post=[batch:batch-… passive]
13:26:54 replay answered elsewhere (all gone): withdraw=["decision:…", "batch-…", "batch-…"]
13:26:54 done after 20.0 s active=false key=none frontmost=theuth — quitting
```

Read: the live engine had 24 items (17 review, 6 decision, 1 blocked; 6 blocking, 18 deferrable) and no new item
arrived during the 20 s, so the live policy stayed on its seeded flood reading and posted nothing (`would-have-posted=[:]`).
The replay — the same live items fed to a fresh policy — shows every class verdict, the held batch at a breakpoint, the
2-minute cap/rate window and withdrawal. Nothing was posted to Notification Center (dry run), `getNotificationSettings`
read `.notDetermined` with no dialog ("not asked yet" in the window), no write RPC ran, the app never became active
(`active=false key=none` at both checkpoints; frontmost stayed the user's app), it quit itself after 20 s and `pgrep -x
Companion` was empty afterwards. `~/.theuth/companion.json` was written once with `{ "launch_at_login": false }`.

**Screenshot** `/tmp/130-w2-settings.png` (`screencapture -x -o -l 1098` from the shell while the window was up): the
Settings window, Notifications tab — three pickers (Immediately · At a break · Immediately), "Sound on errors" checked,
"Enable notifications — not asked yet", the first-run explanation, "Launch at login" unchecked with its note. The Rail
tab was captured on an earlier run (13:24). The first run (13:21) tried `screencapture` from inside the app: exit 1 —
the companion has no Screen Recording grant; moved to the shell.

**Not exercised live (by the rules):** a real banner, an action tap and the receipt (needs authorization = a dialog);
the double-tap ⌥ monitor (needs the Accessibility prompt); `SMAppService.register()`. All three are the manual steps below.

### 2.1 Manual steps for the user

1. Build + launch the bundle normally (`just companion`, then `open -g 'target/release/companion/Theuth Companion.app'`;
   or `… --args --request-notifications` to get the dialog at once). Right-click the menu bar glyph → **Settings…** →
   Notifications → **Enable notifications** → Allow in the system dialog; the status reads "on".
2. Have an agent ask something (or wait for one): a banner "w2 · question" with the question's own option buttons and
   **Reply…**; an approval shows **Allow once · Deny · Deny with note…**. Press a button: it is sent at once (no 3 s
   hold), a small receipt banner "w2 · <label> — sent" follows, and the rail count drops. Answer another item from
   theuth itself: its banner disappears. Tap a banner body: the panel opens on that item.
3. Keys → **Double-tap ⌥ as well**: the Accessibility prompt appears once; allow it in System Settings, flip the toggle
   again; then ⌥⌥ in any app toggles the panel (the chord keeps working regardless).
4. **Launch at login**: toggle it (menu or Settings) → `~/.theuth/companion.json` = `{ "launch_at_login": true }`.
   Restart theuth: the log shows `companion: login item registered` then `status=Enabled` (or `requires_approval` →
   flip the switch in System Settings › General › Login Items). While enabled, theuth no longer launches the helper via
   NSWorkspace — launchd does. The main UI can read `invoke('companion_login_item_status')`.
5. Menu → **Pause notifications ▸ 1 hour**: nothing is posted, the rail stays live; **Off** resumes.

## 3. Deviations from the design and the brief

- **First snapshot is seeded, not posted.** The design classifies "every new attention item"; the first `attention_list`
  after launch (24 items here) is treated as already-known so a relaunch never fires a flood summary or 24 banners.
  Only the flood state is taken from it. Items that appear later are new.
- **`blocked` is deferrable.** The §4.4 table does not list it; it is needs-you without a prompt, so it rides with reviews.
- **Two categories beyond the five:** `error` (Open) for the error class, and plain kinds without card options
  (hire · blocked · budget 80 % · secret · handoff) reuse `batch` (Open panel · Later). Per-banner category variants
  (`question:<id>` …) carry the real option titles; the five base categories are still registered on every launch.
- **"Allow for session"** is not an action (R1-5: `Decision` is allow/deny); "Deny with note…" sends deny then the note
  as `send_input` to the item's session.
- **Error class = budget paused · node unreachable · engine lost.** "Failed job" has no attention kind on today's wire;
  agent `error` states stay a rail mark.
- **Settings is a plain `TabView` window**, not sindresorhus/Settings (one less dependency). Rail side/display changes
  apply on the rail's next layout (W1 reads `Settings.railSide` at layout time; `railDisplay` is persisted but W1's
  rail still uses `NSScreen.screens.first` — a one-line change in `RailPanel.layout` when W1's file is open again).
- **No `.provisional` first** (§5.7): the request is only ever user-initiated, so it asks for the real alert permission.
- **Login item status is a string command** (`companion_login_item_status() -> String`, snake_case), not a struct.
- **`companion.json` is written on first launch with `false`** so the shell always finds a file; the shell treats a
  missing file as `false` anyway.
- **Hub** in Settings › Engine is a placeholder line — the hub connection is W3's.

## 4. Open items

- Live banner · tap · receipt · withdrawal and the double-tap monitor need the two user-initiated prompts (§2.1).
- Integration (not this wave): `UI/App.tsx` `notifyIfHidden` + tray `●N` suppression while `companion-alive`;
  showing `companion_login_item_status` in the main UI; the release Mac's App ID + provisioning profile for
  `com.apple.developer.usernotifications.time-sensitive` → then `Settings.timeSensitiveAvailable` (today a constant
  `false` in `NotificationPrefs`) can be flipped and the entitlement added to `companion.entitlements.plist`.
- W3 merge points: `AppDelegate.swift:45/116/167-176`, `Store.swift:267-275`, the status menu (W3's "Ask theuth…"
  should insert before the same "— · Quit" pair), `Flags`/`Settings` extensions live in `Settings/SettingsStore.swift`.
- `BreakpointSource` reads idle time every second whether or not anything is held (cheap IOKit registry read); §5.5's
  "every 5 s while holding" would save nothing measurable. The replay in `--probe-notify` is diagnostic only.
