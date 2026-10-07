# Spike SA — S1 SwiftPM-built .app bundle without Xcode + S2 notifications from that bundle

Status: DONE 12:10 (skeleton 11:48; last save 12:10). Worker: spike-130-SA. Work dir: /tmp/130-spikes/SA/.
Machine: macOS 26.6.2 (25G83, Tahoe), Command Line Tools only (no xcodebuild), Swift 6.3.2, SDK 26.5. tauri-cli 2.11.4 installed (`~/.cargo/bin/cargo-tauri`), which embeds tauri-bundler 2.9.4 + tauri-macos-sign 2.3.4 (sources extracted from `~/.cargo/registry/cache/*/<crate>.crate` into /tmp/130-spikes/SA/bundler-src/ — the bundler is not in registry/src because it was `cargo install`ed; cite paths below are relative to that dir).

## 1 SwiftPM app (build facts, workarounds)

Package: /tmp/130-spikes/SA/CompanionSpike (Package.swift + Sources/CompanionSpike/main.swift, 1 file, ~150 lines).
Target: `.executableTarget("CompanionSpike")`, AppKit + SwiftUI + UserNotifications, top-level `main.swift`
(NSApplication.shared → `setActivationPolicy(.accessory)` → NSApplicationDelegate builds the NSStatusItem (template
SF Symbol `circle.hexagongrid.fill`, 1-item menu) and the borderless `NSPanel([.borderless, .nonactivatingPanel])`,
level `.statusBar`, `collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]`, `sharingType = .none`,
`ignoresMouseEvents = true`, 26×200 pt at `NSScreen.main.frame.maxX - 26`, hosting `NSHostingView(rootView: RailView())`
with 6 circles (#c47a2c, #5b8f8a, #4a8a6e, #c45a48, #5b8f8a, dashed #c47a2c), `orderFrontRegardless()`, then
`DispatchQueue.main.asyncAfter(deadline: .now() + 6) { NSApp.terminate(nil) }`).

Build facts (command + output, run 2026-10-07 11:49 while other sessions were compiling Rust on the same machine):
- `cd /tmp/130-spikes/SA/CompanionSpike && time swift build -c release` → `Build complete! (67.60s)`;
  `real 1m9.067s user 0m51.580s sys 0m6.344s`. Incremental rebuilds after editing main.swift: 3.36 s, 2.51 s, 4.77 s. A clean release build of ONE Swift file; ~52 s of that is CPU
  (SwiftUI/AppKit module interface loading + optimisation) — expect ~20–30 s on an idle machine, unverified.
- Binary: `.build/release/CompanionSpike` = 152 544 bytes (149 KiB) for the step-1 program; 155 152 bytes with file logging; 180 928 bytes (177 KiB) with the §8.1 manual-probe code (final), `Mach-O 64-bit executable arm64` (no fat/x86_64
  slice — SwiftPM builds the host arch only; a universal binary needs `swift build --arch arm64 --arch x86_64`, unverified here).
- Toolchain: Command Line Tools only, `swift-driver 1.148.6 / Apple Swift 6.3.2 / Target arm64-apple-macosx26.0`.

Workarounds / things that need or do not need Xcode:
1. `platforms: [.macOS(.v15)]` is NOT available with `// swift-tools-version: 5.9` — the manifest fails with
   `error: 'v15' is unavailable … 'v15' was introduced in PackageDescription 6.0` (output of the first build attempt).
   Fix: `// swift-tools-version: 6.0` + `swiftSettings: [.swiftLanguageMode(.v5)]` on the target so the AppKit
   top-level code compiles without Swift-6 strict-concurrency annotations. The brief's "5.9 or newer" must read "6.0".
2. Info.plist: SwiftPM does not generate one for an executable target and there is no `-Xlinker -sectcreate __TEXT
   __info_plist` trick needed — the bundle script writes `Contents/Info.plist` by hand (section 2). Nothing Xcode-only.
3. Asset catalogs (.xcassets → Assets.car) need `actool`, which is in Xcode, not in the CLT (`xcrun --find actool`
   fails on this machine, see section 2). Not needed: the status-item image is an SF Symbol
   (`NSImage(systemSymbolName:accessibilityDescription:)`, template), colours are literal `Color(red:green:blue:)`.
   App icon is produced without Xcode via `iconutil` from PNGs (section 2) — `iconutil` IS in the CLT.
4. No storyboard/xib (`NSMain`/`NSPrincipalClass` not needed): `NSApplication.shared` + `app.run()` from `main.swift`.
5. Nothing else in the brief's feature list (NSStatusItem, non-activating NSPanel, NSHostingView, UNUserNotificationCenter
   categories) needs Xcode; `xcodebuild` was never invoked.

## 2 Bundle script and run (screenshot verdict, codesign output)

Script: /tmp/130-spikes/SA/bundle.sh (builds, assembles `out/Theuth Companion Spike.app`, draws the icon, signs, verifies).
Full output: /tmp/130-spikes/SA/bundle-run2.log. Bundle layout produced (`find … -type f`):
`Contents/MacOS/CompanionSpike` · `Contents/Info.plist` (CFBundleIdentifier dev.theuth.companion.spike, CFBundleName/
CFBundleDisplayName "Theuth Companion Spike", LSUIElement true, CFBundleShortVersionString 0.1.0, CFBundleVersion 1,
NSHumanReadableCopyright, LSMinimumSystemVersion 15.0, NSPrincipalClass NSApplication, CFBundleIconFile AppIcon) ·
`Contents/PkgInfo` (`APPL????`) · `Contents/Resources/AppIcon.icns` (375 131 bytes) · `Contents/_CodeSignature/CodeResources`.
`plutil -lint Info.plist` → OK.

Icon without Xcode: `xcrun --find actool` fails on this machine ("actool: NOT available (Xcode only)" in the log), so the
icon is a 1024² PNG drawn by /tmp/130-spikes/SA/icon.swift (AppKit NSBezierPath → NSBitmapImageRep PNG, run with the
`swift` interpreter), resized by `sips -z` into AppIcon.iconset (16…512 + @2x) and packed with `iconutil -c icns`
(iconutil IS in the CLT). Works; `.icns` is referenced from Info.plist. Optional for the real companion (an SMAppService
login item shows the helper's name/icon in System Settings › Login Items, so it is worth having).

Signing: `xattr -crs "$APP"` then `codesign -s - --force --deep "$APP"` ("replacing existing signature" — the linker's
ad-hoc signature on the SwiftPM binary). `codesign -dv --verbose=2` output (verbatim):
```
== codesign -dv --verbose=2
Executable=/private/tmp/130-spikes/SA/out/Theuth Companion Spike.app/Contents/MacOS/CompanionSpike
Identifier=dev.theuth.companion.spike
Format=app bundle with Mach-O thin (arm64)
CodeDirectory v=20400 size=531 flags=0x2(adhoc) hashes=10+3 location=embedded
Signature=adhoc
Info.plist entries=15
TeamIdentifier=not set
Sealed Resources version=2 rules=13 files=1
Internal requirements count=0 size=12
== codesign --verify --deep --strict --verbose=2
/tmp/130-spikes/SA/out/Theuth Companion Spike.app: valid on disk
/tmp/130-spikes/SA/out/Theuth Companion Spike.app: satisfies its Designated Requirement
verify: OK
```
`codesign --verify --deep --strict --verbose=2` → `valid on disk` / `satisfies its Designated Requirement`.

Runs (all via `open -g 'out/Theuth Companion Spike.app' [--args …]`; the app logs to /tmp/130-spikes/SA/companion-run.log
because `open` gives it no stdout; each run quit itself 6 s after launch — `pgrep CompanionSpike` empty 8.5 s after `open`):
- 11:54:10 pid 36020 (first run): `applicationDidFinishLaunching; bundlePath=/private/tmp/130-spikes/SA/out/Theuth Companion
  Spike.app bundleId=dev.theuth.companion.spike` → `status item created` → `panel ordered front at (2534.0, 620.0, 26.0, 200.0)
  on screen (0.0, 0.0, 2560.0, 1440.0); isVisible=true level=3` → `6 s elapsed — terminating`. The panel was NOT in the
  screenshot (shot.png). Two causes found and fixed: (a) `isFloatingPanel = true` set AFTER `level = .statusBar` resets
  the level to `.floating` (3) — removed, level now 25; (b) **`sharingType = .none` excludes the window from
  `screencapture`** (see below). Not a bug in the panel.
- 11:58:56 pid 48720 RUN A (`sharingType = .none`, level=25 sharingType=0 windowNumber=367) and 11:59:48 pid 53784 RUN D (same):
  /tmp/130-spikes/SA/windows (CGWindowListCopyWindowInfo, .optionOnScreenOnly, filtered to our own app) lists
  `owner=Theuth Companion Spike pid=53784 window=380 layer=25 alpha=1.0 sharingState=0 bounds=[X 2534, Y 620, W 26, H 200]`
  — the panel IS on screen at the right edge, at status-bar level — but `screencapture -x shot-A.png` shows nothing at
  that place (crop identical to the before-launch screenshot; SVG /tmp/130-spikes/SA/shot-compare.svg).
- 11:59:05 pid 49382 RUN B and 11:59:3x pid 52674 RUN C (`--shareable` → `sharingType = .readOnly`, sharingState=1, same bounds):
  `screencapture -x shot-C.png` SHOWS the 26×200 dark rounded rail with the six circles (orange #c47a2c, teal, green, red,
  teal, dashed orange) at x 2534–2560, y 620–820 — exactly the panel rect. Screenshot verdict: **status item and panel
  appeared** (status item: a new template hexagon-grid icon appears at the left end of the menu-bar extras in
  mb-compare.svg, absent in the before-launch strip; panel: shot-compare.svg right pane).
- Side finding, relevant to the design: on macOS 26.6.2 `NSWindow.sharingType = .none` hides the rail from
  `screencapture`/screen capture entirely (and so from theuth's own `computer_screenshot`, Zoom, etc.) while
  CGWindowList still reports it on screen. That is the desired behaviour for a privacy rail during screen-shares, but it
  means (1) the companion's screenshots-based self-checks must use a `.readOnly` debug switch, and (2) the agent's own
  screenshots will never show the rail. The knowledge base note `facts/macos-screen-capture-exclusion-sharingtype` said
  this was "contested on macOS 15+"; here it is observed working on 26.6.2 for a plain `screencapture -x`.
- No TCC dialog, no activation (accessory policy, `open -g`, `orderFrontRegardless`), no focus change was observed; the
  frontmost app in the screenshots stayed the user's (Ghostty menu bar).
- The status-item window is not listed by CGWindowList as a separate on-screen window of the app (only the panel is) —
  the menu-bar strip comparison is the evidence for it. Unverified why (likely hosted differently on macOS 26).

## 3 Tauri bundler findings (file:line) and the recommended recipe

Sources: tauri-bundler 2.9.4 (`B/` = /tmp/130-spikes/SA/bundler-src/tauri-bundler-2.9.4/), tauri-macos-sign 2.3.4 (`S/`),
tauri-utils 2.9.3 (`U/`), all the versions embedded in the installed tauri-cli 2.11.4 (`cargo-tauri tauri --version`
→ `tauri-cli 2.11.4`; `~/.cargo/.crates.toml`). theuth: `crates/desktop/tauri.conf.json`, `tauri.macos.conf.json`,
`tauri.bundle.conf.json`, `justfile:107-122`, `scripts/package-macos.sh`, `scripts/notarize-macos.sh`, `scripts/load-signing-env.sh`.

How `bundle.macOS.files` copies into Contents/:
- Config: `U/src/config.rs:613-620` `MacConfig.files: HashMap<PathBuf, PathBuf>` — "The files to include in the application
  relative to the Contents directory" (key = destination under Contents/, value = source path).
- Copy: `B/src/bundle/macos/app.rs:183-209` `copy_custom_files_to_bundle`: source must exist (`:185-189`), an absolute key has
  its leading `/` stripped (`:191-195`), a file → `fs_utils::copy_file` (`:196-198`), a **directory → `fs_utils::copy_dir`
  (recursive)** (`:199-201`), anything else is an error (`:202-205`). So `"Library/LoginItems/Theuth Companion.app":
  "../companion/.build/Theuth Companion.app"` lands the whole helper bundle at `Contents/Library/LoginItems/…`.
- `fs_utils::copy_dir` (`B/src/utils/fs_utils.rs:103-135`) walks with `walkdir`, recreates **symlinks as symlinks**
  (`:121-127`, `read_link` + `std::os::unix::fs::symlink`), creates dirs (`:128-129`) and copies files with `std::fs::copy`
  (`:131`), which on Unix copies the permission bits — **executable bits are preserved** (verified by the Rust std contract
  of `fs::copy`; not re-tested here). Call order in `bundle_project` (`app.rs:55-154`): icon → Info.plist (`:85-91`) →
  frameworks (`:93-95`) → resources (`:97`) → external binaries into Contents/MacOS (`:99-105`) → main binaries (`:107-111`)
  → **custom files (`:113`)** → signing (`:115-151`).

Signing order and what the bundler signs:
- The bundler signs ONLY when `keychain()` returns Some — i.e. `APPLE_CERTIFICATE`+`APPLE_CERTIFICATE_PASSWORD` are set or
  `bundle.macOS.signingIdentity` is configured (`S/../sign.rs:19-44`; identity key `U/src/config.rs:653`). With neither,
  `sign_paths` is dropped and NOTHING is signed (`app.rs:115-119`: `--no-sign` warns, `None` keychain silently skips).
- When it signs: `sign_paths` = frameworks (and their nested code, `app.rs:445-465`) + external binaries + main binaries,
  then the .app itself is pushed LAST (`app.rs:120-125`, comment "signing must be done inside out") and each target gets one
  `codesign --force -s <identity> [--options runtime] [--keychain …] [--entitlements …] <path>`
  (`S/src/keychain.rs:207-249`; `--options runtime` only when `is_an_executable && hardened_runtime`, `sign.rs:64-70`).
  `xattr -crs <app>` runs before signing (`app.rs:127-129, 156-163`).
- **`--deep` appears nowhere in the signing code** — the only `--deep` in the whole bundler is the DMG script's
  `codesign --verify --deep` (`B/src/bundle/macos/dmg/bundle_dmg:613`). Nested `.app`/`.xpc`/`.framework` discovery
  (`add_nested_code_sign_path`, `app.rs:488-530`, folders `NESTED_CODE_FOLDER = MacOS, Frameworks, Plugins, Helpers,
  XPCServices, Libraries`, `app.rs:44-51`) is reached ONLY from `copy_frameworks_to_bundle` (`app.rs:404`) — **custom
  `files` are never added to `sign_paths`** (`copy_custom_files_to_bundle` returns `()`), and `Library/LoginItems` is not
  in `NESTED_CODE_FOLDER` anyway. Consequence: a companion placed via `bundle.macOS.files` is NOT signed by the bundler;
  and `Contents/Library/LoginItems/` IS a standard nested-code location for codesign (the sealed CodeResources rule
  `^(Frameworks|SharedFrameworks|PlugIns|Plug-ins|XPCServices|Helpers|MacOS|Library/(Automator|Spotlight|LoginItems))/`
  with `nested = true`, read from the test bundle below), so signing the outer app over an UNSIGNED companion FAILS
  outright — verified here (/tmp/130-spikes/SA/nest, outer `Outer.app` with the spike app nested, signature removed):
  `codesign -s - --force nest/Outer.app` → `code object is not signed at all / In subcomponent: …/Contents/Library/LoginItems/
  Theuth Companion Spike.app`, exit 1 (T1). Signing the companion first, then the outer app non-deep (T2) → `--prepared:` /
  `--validated:` lines for the nested app, `valid on disk`, `satisfies its Designated Requirement`, and CodeResources carries
  `Library/LoginItems/Theuth Companion Spike.app` with its `cdhash` + `requirement`. `codesign --deep` on the outer app over
  an unsigned companion (T3) also works and leaves the nested app signed (`Identifier=dev.theuth.companion.spike
  Signature=adhoc`) — fine for ad-hoc dev builds, but for Developer ID the explicit inner line is preferred (Apple
  discourages `--deep` for distribution: it applies the outer flags/entitlements to every nested item).
- Notarization inside the bundler (`app.rs:134-150`, `sign.rs:96-160`) is of the .app and only with `APPLE_ID`/
  `APPLE_PASSWORD`/`APPLE_TEAM_ID` or `APPLE_API_KEY`/`APPLE_API_ISSUER`[/`APPLE_API_KEY_PATH`]; `MissingTeamId` is fatal.

How theuth signs and notarizes TODAY (so the bundler's signing path is irrelevant to it):
- `just package-macos` → `scripts/package-macos.sh`: `cargo tauri build --config tauri.bundle.conf.json` (`:87`;
  `tauri.bundle.conf.json` sets `externalBin: ["binaries/t"]`, `hardenedRuntime: true`, `entitlements: ./entitlements.plist`,
  **no `signingIdentity`** → bundler signs nothing); then the script adds `libonnxruntime.dylib` to Contents/Frameworks
  (`:92-98`) and two Bun helpers to Contents/MacOS (`:100-103`), then signs **inside-out by hand** (`:109-128`):
  `codesign --force [--timestamp|--timestamp=none] --sign "$IDENTITY"` for the dylib, the two helpers (`--options runtime`,
  `-i io.theuth.mac.<helper>`, helper entitlements), the `t` sidecar (`-i io.theuth.mac.t`, app entitlements), then the .app
  (`--options runtime --entitlements crates/desktop/entitlements.plist`), then `codesign --verify --deep --strict --verbose=2`
  (`:128`) and `spctl --assess` (`:131`). Identity: `THEUTH_SIGN_IDENTITY` (default `-` ad-hoc), optionally from repo-root
  `.env.signing` via `scripts/load-signing-env.sh`. The DMG is built AFTER signing (`:133-142`, comment: Tauri's dmg target
  would snapshot pre-signature) and signed when the identity is not ad-hoc.
- `just notarize-macos` → `scripts/notarize-macos.sh`: `xcrun notarytool submit <dmg> --wait` with
  `THEUTH_NOTARY_KEYCHAIN_PROFILE` or `THEUTH_NOTARY_APPLE_ID` + `THEUTH_NOTARY_TEAM_ID` + `THEUTH_NOTARY_PASSWORD` (`:28-35`),
  then `xcrun stapler staple` and `spctl --assess --type open` (`:38-42`). The notarization ticket covers everything
  inside the DMG's .app, nested bundles included — provided every Mach-O inside is Developer-ID signed with hardened
  runtime + secure timestamp.

Recommended recipe for the nested companion (minimal change to the existing pipeline):
1. Build: a `companion/` SwiftPM package in the repo; `swift build -c release [--arch arm64]` + a `scripts/bundle-companion.sh`
   (this spike's bundle.sh generalised: Info.plist with `CFBundleIdentifier io.theuth.mac.companion` — a DIFFERENT id from
   `io.theuth.mac`, `LSUIElement true`, `CFBundleShortVersionString` = app version, `LSMinimumSystemVersion 15.0`,
   `CFBundleIconFile`) → `target/release/companion/Theuth Companion.app`, UNSIGNED or ad-hoc (its signature is replaced in
   step 3). No Xcode anywhere. Run it from `scripts/package-macos.sh` step 1 beside the sidecar build.
2. Place: either `"files": {"Library/LoginItems/Theuth Companion.app": "../../target/release/companion/Theuth Companion.app"}`
   under `bundle.macOS` in `tauri.bundle.conf.json` (paths in that file resolve relative to crates/desktop, same as
   `externalBin`; the bundler copies the directory recursively with symlinks and exec bits intact — `copy_dir`), **or** a
   `ditto` into `$APP/Contents/Library/LoginItems/` in package-macos.sh right after the helpers (`:100-103`). Prefer the
   config route (the bundle is complete at `cargo tauri build` time, so `cargo tauri dev`/ad-hoc runs also have the helper)
   but note `cargo check -p desktop` does not read tauri.bundle.conf.json, so the file may be absent at check time — the
   bundler errors only at bundle time if the source is missing (`app.rs:185-189`).
3. Sign, inside-out, in package-macos.sh BEFORE the outer `.app` line (`:126-127`), i.e. a new block after the `t` sidecar:
   `codesign --force --options runtime $TIMESTAMP_FLAG --sign "$IDENTITY" --entitlements <companion entitlements or none>
   "$APP/Contents/Library/LoginItems/Theuth Companion.app"` — a single non-deep sign of the helper bundle is enough because
   the helper has one Mach-O and no nested code; it uses the helper's own Info.plist identifier (no `-i` needed). Then the
   existing outer `codesign … "$APP"` seals it as nested code, `codesign --verify --deep --strict` (`:128`) verifies both,
   and the existing `notarize-macos.sh` notarizes both via the DMG. **No pre-bundle signing step is needed** (the bundler
   never signs in theuth's flow; signing the helper before copying would be re-done anyway) — but it does not hurt for dev
   runs to ad-hoc sign it in step 1 so `open`/SMAppService can launch it un-notarized.
4. If the bundler's own signing is ever switched on (APPLE_CERTIFICATE in CI), the companion would still be unsigned by it
   (custom files never enter `sign_paths`); keep the explicit codesign line in the script, or move the helper under
   `Contents/Helpers/` — no: SMAppService requires `Contents/Library/LoginItems/` (section 4), so the explicit line stays.
5. Hardened runtime on the helper: required for notarization; the helper needs no special entitlements for
   NSStatusItem/NSPanel/UNUserNotificationCenter (verified: this spike ran all three ad-hoc with no entitlements); a global
   hotkey via `CGEvent` tap would need Input Monitoring (TCC, user-granted at runtime, not an entitlement); App Intents /
   WidgetKit later would need an app-group entitlement shared with `io.theuth.mac` — out of scope here.
6. Universal binary: theuth packaging is arm64-only ("Arm64-only: built on/for the host", package-macos.sh:17) — the
   companion stays arm64 too; `swift build --arch arm64 --arch x86_64` would be the universal path if that ever changes.

## 4 SMAppService facts

Read from Apple's DocC data for the pages (all fetched 2026-10-07; `read_url` of the `/tutorials/data/documentation/…json`
form of each page, text extracted — the HTML pages are JS-rendered):
- https://developer.apple.com/documentation/servicemanagement/smappservice — "An object the framework uses to control
  helper executables that live inside an app's main bundle." "In macOS 13 and later, use SMAppService to register and
  control LoginItems, LaunchAgents, LaunchDaemons as helper executables for your app."
- https://developer.apple.com/documentation/servicemanagement/smappservice/loginitem(identifier:) — `class func loginItem(
  identifier: String) -> SMAppService`; parameter: "**The bundle identifier of the helper application.**" Discussion
  (Apple's wording, verbatim): "The property list name must correspond to a property list in the calling app's
  **Contents/Library/LoginItems** directory" — i.e. the helper .app must be at `<main>.app/Contents/Library/LoginItems/`
  and is addressed by ITS OWN bundle identifier (so the companion needs a CFBundleIdentifier distinct from `io.theuth.mac`,
  e.g. `io.theuth.mac.companion`; the system also attributes it to the main app in System Settings › Login Items because
  it lives inside the main bundle — "Every app that supports a [login item] has a corresponding switch in the Login Items
  panel in System Settings", from the "Updating helper executables from earlier versions of macOS" article).
- https://developer.apple.com/documentation/servicemanagement/smappservice/register() — "Registers the service so it can
  begin launching subject to user approval." Rules: "**If the service corresponds to a LoginItem bundle, the helper starts
  immediately and on subsequent logins. If the helper crashes or exits with a non-zero status, the system relaunches it.**"
  "If the service is already registered, this method returns kSMErrorAlreadyRegistered" [the doc's inline reference;
  the Status page adds: `notRegistered` also covers "the service attempted to reregister after it was already
  registered"]; "If the service isn't approved by the user, kSMErrorLaunchDeniedByUser" ("The user denied the app's launch
  request."). Errors: `kSMErrorAlreadyRegistered` "The application is already registered.", `kSMErrorLaunchDeniedByUser`.
- https://developer.apple.com/documentation/servicemanagement/smappservice/status-swift.enum — `.enabled` "successfully
  registered and is eligible to run"; `.requiresApproval` "successfully registered, but the user needs to take action in
  System Preferences"; `.notRegistered`; `.notFound` "An error occurred and the framework couldn't find this service"
  (= wrong location/identifier — the error you get when the helper is not under Contents/Library/LoginItems or the id does
  not match its Info.plist). `SMAppService.openSystemSettingsLoginItems()` opens the panel for the user.
- https://developer.apple.com/documentation/servicemanagement/updating-helper-executables-from-earlier-versions-of-macos —
  "If your app can't run or operate correctly without helper executables, check the authorization status at launch. If
  helper executables don't have authorization, alert the user, and call [openSystemSettingsLoginItems] …"; macOS attributes
  the helper to the app's name in Login Items because it is inside the bundle (legacy plists need `AssociatedBundleIdentifiers`
  and a matching Team Identifier — not our case, but it means a Developer-ID TEAM match between outer app and helper is
  expected; ad-hoc dev builds are attributed by executable name).

What `register()` does for an ALREADY-RUNNING helper: the docs do not say. Known: it "starts immediately" the helper on
registration and relaunches it on crash/non-zero exit; a second `register()` returns the already-registered error (status
stays `.enabled`). Whether launchd starts a second instance when the same bundle is already running from `open`/Finder is
**unverified** here (not called, per the rules). Design consequences: (a) the main app registers the login item (idempotent:
treat `kSMErrorAlreadyRegistered` as success) and never launches the helper itself; the helper's lifecycle belongs to
launchd (crash → relaunch for free); (b) the helper should exit 0 on a duplicate instance (`NSRunningApplication.
runningApplications(withBundleIdentifier:)` count > 1 at launch → quit) so an `open` during development cannot double it;
(c) "exits with a non-zero status → relaunch" means the helper must exit 0 when told to quit by the main app, and
`unregister()` is the way to stop it for good; (d) status `.requiresApproval` must be surfaced in theuth's Settings with a
button that calls `openSystemSettingsLoginItems()`. Nothing here needs Xcode; `SMAppService` is in the ServiceManagement
framework, callable from Rust/objc2 in the Tauri app or from Swift in the helper — the registration call must come from
the MAIN app's process (it names the helper by id inside its own bundle).

Manual step (not run — it registers a login item on the user's Mac and flips the Login Items switch): see §8 item 3.

## 5 Notification probe results

Probe code: `notifyProbe()` in main.swift, only on `--notify-probe`; it calls `UNUserNotificationCenter.current()`,
`setNotificationCategories([question])` where `question` = 3 × `UNNotificationAction` (approve / deny[.destructive] /
open[.foreground]) + 1 × `UNTextInputNotificationAction` (reply, button "Send", placeholder "Answer"), options
`.customDismissAction`; then `getNotificationCategories` and `getNotificationSettings`. **No `requestAuthorization`** was
called in any run: the only call sits in the manual-only `--request-auth` path added afterwards for §8.1 (never passed by
this spike; the final default run at 12:08 was re-checked: `grep -c request-auth companion-run.log` → 0). No dialog appeared in any run.

Results (logs: /tmp/130-spikes/SA/probe-bundle.log, probe-bare.log, companion-run.log; 2026-10-07 12:01):
1. From the bundle, executable run directly (`'out/Theuth Companion Spike.app/Contents/MacOS/CompanionSpike' --notify-probe`),
   pid 58821, exit 0:
   `Bundle.main.bundleIdentifier = dev.theuth.companion.spike` · `bundle name the system would show (CFBundleDisplayName ??
   CFBundleName) = Theuth Companion Spike` · `got center <UNUserNotificationCenter: 0x7d50204b0>` · `setNotificationCategories
   done` · `getNotificationCategories -> []` · **`authorizationStatus = .notDetermined (raw 0)`** · `alertSetting=0
   soundSetting=0 badgeSetting=0 alertStyle=0` · `done (callback received)`.
2. From the bundle via LaunchServices (`open -g 'out/Theuth Companion Spike.app' --args --notify-probe`), pid 58856: identical
   output, `.notDetermined`, exited by itself.
3. Bare executable (`CompanionSpike/.build/release/CompanionSpike --notify-probe`), pid 58825, **exit 134 (SIGABRT)**:
   `Bundle.main.bundleIdentifier = nil`, then at `UNUserNotificationCenter.current()`:
   `*** Terminating app due to uncaught exception 'NSInternalInconsistencyException', reason: 'bundleProxyForCurrentProcess is
   nil: mainBundle.bundleURL file:///private/tmp/130-spikes/SA/CompanionSpike/.build/release/'` — thrown inside
   `+[UNUserNotificationCenter currentNotificationCenter]` (frames 3–7 of the backtrace). Exactly the predicted failure mode:
   UNUserNotificationCenter needs a real .app bundle; `swift run` / the bare SwiftPM binary cannot be used to test it.

Reading of the results:
- An ad-hoc-signed, SwiftPM-built, hand-assembled bundle (no Xcode, no provisioning profile, no entitlements) can
  construct the center, register a category with 3 buttons + a text-input action, and read its settings without crashing
  and without any prompt. Status `.notDetermined` is what the brief expected (nothing has asked for permission).
- `getNotificationCategories` came back EMPTY right after `setNotificationCategories`. Cause unverified; the likely
  explanation is that usernoted does not persist categories for a bundle that has never been authorised (status
  `.notDetermined`) — re-check in the manual step after authorisation (`getNotificationCategories` should then list
  `question`). Design note: register categories on every launch (Apple's recommended pattern) and do not rely on
  read-back before authorisation.
- Delivering a notification (`add(UNNotificationRequest)`) and receiving `didReceive response` with
  `actionIdentifier`/`userText` was NOT exercised here (would require authorisation = a dialog). Manual step in §8.
- Prerequisite for the real companion: the helper must run FROM its bundle (SMAppService launches it that way; `open`
  does; a debugger launching the bare binary does not).

## 6 Tauri plugin verdict

Source read: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tauri-plugin-notification-2.4.0/src/` (the version theuth's
`crates/desktop/ui/package.json` pins as `@tauri-apps/plugin-notification ^2.4.0`; node_modules has 2.4.0) plus its macOS
backend `notify-rust-4.18.0` and `mac-notification-sys-0.6.15` from the same registry.

Verdict: **actions / `onAction` are NOT supported on macOS desktop** (nor on any desktop) by the plugin as shipped.
- Desktop command surface is three commands only: `commands::notify`, `commands::request_permission`,
  `commands::is_permission_granted` — `src/lib.rs:228-232` (`Builder::new("notification").invoke_handler(generate_handler![…])`).
  `register_action_types` exists only in `src/mobile.rs:66` (`pub fn register_action_types(&self, types: Vec<ActionType>)`),
  behind `#[cfg(mobile)]` (`src/lib.rs:238-241`).
- Desktop `NotificationBuilder::show()` — `src/desktop.rs:179-221` — builds a `notify_rust::Notification` with only `body`,
  `summary` (title), `icon`/`auto_icon`, `sound_name`; the builder's `action_type_id` (`src/lib.rs:119-121`) and any
  `actions` are never passed to notify-rust. On macOS it first calls `notify_rust::set_application(if tauri::is_dev()
  {"com.apple.Terminal"} else {&self.identifier})` (`src/desktop.rs:207-214`) and then `tauri::async_runtime::spawn(async
  move { let _ = notification.show(); })` (`:216-218`) — the result/response is discarded, so nothing could ever be
  reported back even if the backend had buttons.
- Permission on desktop is a constant: `request_permission()` and `permission_state()` both return
  `Ok(PermissionState::Granted)` (`src/desktop.rs:61-67`) — no `UNUserNotificationCenter.requestAuthorization` ever runs; the
  real macOS authorisation is whatever the deprecated `NSUserNotificationCenter` path gets implicitly.
- Backend: `notify-rust 4.18.0` default features → `src/macos/mod.rs:1-11` uses the **`NSUserNotificationCenter`** backend
  (`nsusernotifications.rs`, via mac-notification-sys 0.6.15, deprecated API since macOS 11). The
  `UNUserNotificationCenter` backend with `.action(Action::button(id,label))` and `wait_for_action` exists in notify-rust
  only behind the opt-in feature `preview-macos-un` (`src/macos/mod.rs:20-38`, `src/macos/unusernotifications.rs:109-125,
  233-237`; `Cargo.toml:67 preview-macos-un = ["dep:mac-usernotifications"]`), which the plugin does not enable
  (`tauri-plugin-notification-2.4.0/Cargo.toml:113-114`: `notify-rust = "4.11"`, no features) and whose actions the plugin
  never fills in anyway.
- JS side (`node_modules/@tauri-apps/plugin-notification/dist-js/index.js`): `registerActionTypes()` → `invoke('plugin:
  notification|register_action_types')` (`:143-144`) — not a desktop command → rejects at runtime on macOS; `onAction(cb)` →
  `addPluginListener('notification', 'actionPerformed', cb)` (`:300-302`) — an event only the mobile plugin emits.
- Confirms the knowledge note `facts/theuth-desktop-shell-companion-seams-2026-10-07` ("notifications are JS {title, body}
  with no actions/click handler"): the Tauri plugin cannot be the vehicle for the Attention panel's actionable
  notifications on macOS. The Swift companion (section 5) is the only path to action buttons + text reply, and it also
  owns the authorisation prompt (which then shows the companion's name "Theuth Companion", not "theuth" — one more reason
  the helper's CFBundleDisplayName/icon matter).

## 7 Verdicts

| Q | Verdict | One line |
|---|---|---|
| S1 — SwiftPM-only build of a menu-bar + non-activating-rail app, no Xcode | **YES** | `swift build -c release` (CLT only, Swift 6.3.2) → 155 KB arm64 binary in 67 s cold / 3 s warm; tools-version must be 6.0 for `.macOS(.v15)`; no API needed Xcode (SF Symbols, hand-written Info.plist, iconutil icon). |
| S1 — bundle it by script and run it | **YES** | bundle.sh → `Theuth Companion Spike.app` (Info.plist, PkgInfo, .icns, ad-hoc `codesign --deep`, `--verify --deep --strict` OK); ran 5× via `open -g`, status item + 26×200 rail at the right edge confirmed on screen (CGWindowList: layer 25, bounds 2534/620/26/200; screenshot with `.readOnly`), quit itself at 6 s, no dialog, no focus steal. |
| S1 — nest it under Tauri's bundler and get it signed/notarized with the outer app | **YES (with one script line)** | `bundle.macOS.files` copies a directory recursively (symlinks + exec bits kept) into Contents/…, but the bundler never signs custom files and theuth's bundler run signs nothing anyway; package-macos.sh already signs inside-out by hand — add ONE `codesign … "$APP/Contents/Library/LoginItems/Theuth Companion.app"` line before the outer app line; `Library/LoginItems` is a standard nested-code location (outer sign FAILS over an unsigned helper — verified), the existing `--verify --deep --strict` + DMG notarytool flow then covers it. No pre-bundle signing step required. |
| S1 — SMAppService facts | **YES (docs)** | `loginItem(identifier:)` takes the HELPER's bundle id, helper must be at `Contents/Library/LoginItems`; `register()` starts it immediately + at login and relaunches on crash/non-zero exit; already registered → `kSMErrorAlreadyRegistered`; behaviour with an already-running instance **UNVERIFIED** (not called). |
| S2 — notifications from a SwiftPM-built, ad-hoc-signed bundle, no dialog | **YES** | From the bundle: `UNUserNotificationCenter.current()`, category `question` (3 actions + text input) and `getNotificationSettings` → `.notDetermined`, exit 0, no prompt. Bare executable: crash `bundleProxyForCurrentProcess is nil` (exit 134) as predicted — always test from the bundle. Delivery + action round-trip needs the dialog → manual step §8.1. |
| S2 — Tauri notification plugin supports actions/onAction on macOS desktop? | **NO** | Desktop plugin exposes only notify / request_permission / is_permission_granted (lib.rs:228-232); `show()` maps title/body/icon/sound to notify-rust's NSUserNotificationCenter backend and discards the result (desktop.rs:179-221); `register_action_types` is mobile-only (mobile.rs:66); permission is a constant `Granted` (desktop.rs:61-67). The Swift companion is the only route to actionable notifications. |

Surprises worth carrying into the design:
1. `NSWindow.sharingType = .none` makes the rail invisible to `screencapture` (and so to theuth's own screenshots and any
   screen share) on macOS 26.6.2 while it is really on screen — keep `.none` for the product, add a debug switch for tests.
2. `isFloatingPanel = true` silently resets `level` to `.floating` (3) — set level last or never set isFloatingPanel.
3. `getNotificationCategories` reads back empty before authorisation — register categories on every launch, do not assert on read-back.
4. The SwiftPM manifest for macOS 15 needs tools-version 6.0 (`.v15` does not exist in 5.9); keep Swift-5 language mode per target.
5. tauri-cli's bundler is not in `~/.cargo/registry/src` (installed binary) — its source is the `.crate` in `registry/cache`.

## 8 Manual steps for the user

1. **Notification authorisation + action round-trip (shows the system dialog; not run):**
   `cd /tmp/130-spikes/SA && rm -f companion-run.log && open -g 'out/Theuth Companion Spike.app' --args --notify-probe --request-auth && tail -f companion-run.log`
   Expect the macOS dialog **“Theuth Companion Spike” Would Like to Send You Notifications** — "Notifications may include
   alerts, sounds, and icon badges. These can be configured in Settings." with **Don't Allow / Allow** (exact wording as on
   macOS 14–15; unverified on 26.6.2). Click Allow → a banner "Theuth Companion Spike · Attention · deploy — Approve the deploy
   of pr-1234 to prod?" appears; hover/long-press it for the buttons **Approve / Deny / Open / Reply…** (Reply… opens the text
   field with the "Send" button). The log then shows `request-auth: granted=true`, `categories now = ["question"]`
   (confirms §5's empty read-back was pre-authorisation), `authorizationStatus raw=2` and `didReceive actionIdentifier=<approve|
   deny|open|reply|com.apple.UNNotificationDefaultActionIdentifier|…DismissActionIdentifier> userText=…`; the app quits 1 s
   after the response (or after 120 s). Afterwards the spike app stays listed in System Settings › Notifications — remove it
   with: `tccutil` does not cover notifications; delete the row in System Settings or ignore it (it is an ad-hoc test bundle).
   Clicking **Don't Allow** → `granted=false`, app quits; re-test needs `rm -rf out/ && ./bundle.sh` with a NEW bundle id
   (edit BUNDLE_ID in bundle.sh) because the denial is remembered per bundle id.
2. **Optional visual re-check of the rail with screen capture enabled (no dialog):**
   `open -g 'out/Theuth Companion Spike.app' --args --shareable; sleep 2; screencapture -x /tmp/130-spikes/SA/shot-manual.png`
   — the 26×200 rail with six dots is at the right edge, vertically centred; it disappears after 6 s. Without `--shareable`
   it is on screen but absent from the capture (§2).
3. **SMAppService registration (registers a login item; not run):** needs a small main-app-side call — e.g. a
   `swift` one-liner cannot do it because the CALLER must be the app whose bundle contains the helper. For the real companion
   this is theuth's Tauri app calling (objc2) `SMAppService.loginItem(identifier: "io.theuth.mac.companion").register()` with a
   companion at `theuth.app/Contents/Library/LoginItems/Theuth Companion.app`; then `status` should be `.enabled` and System
   Settings › General › Login Items & Extensions lists "theuth". For a hand test before that exists: put the spike bundle under
   a copy of theuth.app (`/Applications/theuth.app` copied to /tmp, spike app ditto'ed into `Contents/Library/LoginItems/`,
   re-sign ad-hoc inside-out as in §3 T2) and run, from inside that copied app's process, the register call — no tooling for
   that exists yet, so this is a W1 item, not a user command. **Unverified**: behaviour of `register()` when the helper is
   already running (expect: launchd takes over the existing instance or starts a second one — the helper should exit 0 on a
   duplicate; see §4).
4. **Developer ID signing of the nested helper (release-time, not run):** with `.env.signing` populated, `just package-macos`
   after adding to scripts/package-macos.sh (before the outer `codesign … "$APP"` line):
   `codesign --force --options runtime $TIMESTAMP_FLAG --sign "$IDENTITY" "$APP/Contents/Library/LoginItems/Theuth Companion.app"`
   then `just notarize-macos` as today; check with `codesign --verify --deep --strict --verbose=2 target/release/bundle/macos/theuth.app`
   (expect `--validated:…/Theuth Companion.app`) and `spctl --assess --type execute -vv` after stapling.

Artefacts: /tmp/130-spikes/SA/{CompanionSpike/ (Package.swift, Sources/CompanionSpike/main.swift), bundle.sh, icon.swift,
windows.swift + windows, out/Theuth Companion Spike.app, bundle-run*.log, companion-run.log, probe-bundle.log, probe-bare.log,
shot*.png, shot-compare.svg, mb-compare.svg, nest/ (codesign nesting test), bundler-src/ (extracted tauri-bundler 2.9.4,
tauri-macos-sign 2.3.4, tauri-utils 2.9.3)}. Nothing under ~/Develop/theuth or any worktree was modified; no cargo/bun/vite run.

