# Spike SC — S5 time-sensitive notifications for a Developer ID app, signing identities, release pipeline facts

Status: COMPLETE 2026-10-07 12:25 (skeleton 11:48; §1–2 12:02; §3–4 12:14; §5–6 12:25). Time box ~40 min, all six questions answered; nothing executed outside /tmp/130-spikes/SC, no permission dialogs, no repo changes. Scratch dir: /tmp/130-spikes/SC.
Machine: macOS 26.6.2, Command Line Tools only (no xcodebuild), Swift 6.3.2.

## 1 Identities

Command run 2026-10-07 11:50 on this Mac (login + System keychains):

```
$ security find-identity -v -p codesigning
     0 valid identities found
$ security find-identity -p codesigning          # includes invalid/expired
  Matching identities: 0 identities found
```

Verdict: **NONE.** No `Developer ID Application`, no `Developer ID Installer`, no `Apple Development`, no
`Apple Distribution` identity exists on this machine, valid or otherwise. (`security find-identity` with no policy
lists three X.509-basic identities that are not code-signing certificates: two UUID-named device/MDM certificates and
one "Cyberhaven localhost" — names only, nothing read or exported.) Consequence: every macOS build on this Mac is
ad-hoc (`codesign -s -`), exactly the `package-macos.sh` default (see §2), and nothing built here can be notarized.
Release signing happens on another machine: `scripts/package-ios.sh:33` defaults the team to `8H548FVL95` and
`scripts/package-ios.sh:49-51` greps for an `Apple Distribution` identity of that team — a release Mac that has the
certificates, not this one (unverified which host; the ad-hoc default and the missing `.env.signing` — `ls .env.signing`
→ "No such file", gitignored at `.gitignore:5` — both say this is not it).

## 2 Release pipeline facts (file:line)

Files read: `justfile` (359 lines), `scripts/package-macos.sh` (152), `scripts/notarize-macos.sh` (43),
`scripts/package-update-macos.sh` (61), `scripts/release-s3.sh` (grep), `scripts/load-signing-env.sh` (11),
`scripts/build-remote.sh` (head), `scripts/package-ios.sh`, `scripts/entitlements-app-parser.plist` (15),
`crates/desktop/entitlements.plist` (12), `crates/desktop/Info.plist` (15), `crates/desktop/tauri.conf.json` (74),
`crates/desktop/tauri.macos.conf.json` (44), `crates/desktop/tauri.bundle.conf.json` (11). There is **no
`.github/` directory** in the repo (`ls .github` → "No such file or directory"): no GitHub Actions; every release is
run by hand from `just` recipes.

### 2.1 Where the build runs

- macOS: **locally on "the arm64 release Mac"** — `scripts/release-s3.sh:133` refuses any other host
  (`[[ "$KEY" == "macos-aarch64" ]] || … "release runs on the arm64 release Mac"`). `scripts/build-remote.sh:1-2` builds
  **only Windows or Linux** on a remote ssh host ("Build the Windows or Linux release artifacts on a remote ssh host");
  `justfile:141-151` documents it. Release order per `justfile:144-148` and `justfile:198-203` (`release:`):
  `package-macos → notarize-macos → package-update-macos → release-s3 → tag-release`.
- `just package-macos` = `scripts/package-macos.sh` (`justfile:110-111`); `just notarize-macos` =
  `scripts/notarize-macos.sh` (`justfile:118-119`); `just desktop-bundle` is an alias (`justfile:220`).

### 2.2 How the .app is built and signed (`scripts/package-macos.sh`)

- `:69` `cargo build --release -p theuth` (sidecar `t`), `:72`/`:75` bun-compiled helpers, `:80` sidecar staged as
  `crates/desktop/binaries/t-<host-triple>`, `:86` `cargo tauri build --config tauri.bundle.conf.json` (app bundle only).
- Tauri's own signing is **not** used for the release: no `signingIdentity` key in any `tauri*.conf.json`, and
  `APPLE_SIGNING_IDENTITY` / `APPLE_CERTIFICATE` are never exported by the scripts (grep of `scripts/ justfile
  crates/desktop/*.json` for `APPLE_[A-Z_]+` finds only the iOS/TestFlight `APPLE_API_*` and `APPLE_DEVELOPMENT_TEAM`
  in `package-ios.sh`/`upload-testflight.sh`). With neither set, tauri-bundler's `keychain(None)` returns `Ok(None)`
  and skips signing (tauri-cli-v2.11.4 `crates/tauri-bundler/src/bundle/macos/sign.rs:19-43`; `app.rs:115-132`).
- The script then **codesigns inside-out itself** (`:108-128`): `libonnxruntime.dylib` (`:113`), the two bun helpers
  with `--options runtime` and `scripts/entitlements-app-parser.plist` (`:114-121`), the sidecar `t` with
  `--options runtime -i io.theuth.mac.t --entitlements crates/desktop/entitlements.plist` (`:122-124`), then the
  `.app` itself with `--options runtime --entitlements crates/desktop/entitlements.plist` (`:125-126`), followed by
  `codesign --verify --deep --strict --verbose=2` (`:127`, verify only — `--deep` is never used for signing).
  Identity: `THEUTH_SIGN_IDENTITY`, default `-` (ad-hoc) (`:20`, `:35`); `--timestamp` for a real identity,
  `--timestamp=none` for ad-hoc (`:109-110`). DMG built after signing with `hdiutil` (`:138`) and signed when the
  identity is real (`:139-141`).
- **Hardened runtime: ON** everywhere — `--options runtime` on every executable in `package-macos.sh:114-126`, and
  `tauri.bundle.conf.json:7` `"hardenedRuntime": true` + `:8` `"entitlements": "./entitlements.plist"` for the
  bundler path (tauri-utils 2.9.3 `src/config.rs:654-656` defaults `hardened_runtime` to true anyway).
- `tauri.macos.conf.json:39` targets `["app","dmg"]`, `:41` `minimumSystemVersion: "15.0"`; `tauri.conf.json:6`
  identifier `io.theuth.mac`, `:4` version `2.10.0`, `:42-46` tray icon, `:14` `macOSPrivateApi: true`.

### 2.3 Entitlements the main app has today (`crates/desktop/entitlements.plist`)

| Key | Line | Why (comment in file) |
|---|---|---|
| `com.apple.security.cs.allow-jit` = true | `:6-7` | V8 (obscura) JIT under the hardened runtime |
| `com.apple.security.device.audio-input` = true | `:9-10` | voice notes: hardened runtime denies getUserMedia without it |

That is the whole file — no sandbox, no `com.apple.developer.*` entitlement, no `application-identifier`, no
`team-identifier`, no `aps-environment`, no `com.apple.developer.usernotifications.time-sensitive`. The helpers'
plist `scripts/entitlements-app-parser.plist:12-13` grants only `allow-jit`. `Info.plist:6`, `:9`, `:12` carry the
microphone, audio-capture and screen-capture usage strings (merged by the bundler).

### 2.4 Env vars that drive signing / notarization / updater signing

| Variable | Read at | Purpose |
|---|---|---|
| `THEUTH_SIGN_IDENTITY` | `package-macos.sh:20,35` | codesign identity (default `-` ad-hoc) |
| `THEUTH_NOTARY_KEYCHAIN_PROFILE` | `notarize-macos.sh:28-29` | `notarytool --keychain-profile` (first choice) |
| `THEUTH_NOTARY_APPLE_ID`, `THEUTH_NOTARY_TEAM_ID`, `THEUTH_NOTARY_PASSWORD` | `notarize-macos.sh:31-34` | `notarytool --apple-id/--team-id/--password` (app-specific password) |
| `THEUTH_UPDATE_KEY`, `THEUTH_UPDATE_KEY_PASSWORD` → exported as `TAURI_SIGNING_PRIVATE_KEY[_PATH]`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | `release-s3.sh:124-130` | tauri-plugin-updater minisign key for the update tarballs (not Apple signing) |
| `APPLE_DEVELOPMENT_TEAM` (default `8H548FVL95`), `APPLE_API_KEY_ID`/`APPLE_API_KEY`/`APPLE_API_ISSUER`/`APPLE_API_KEY_PATH` | `package-ios.sh:16,33,73,76`; `upload-testflight.sh:11-14,36-52` | iOS only |
| `.env.signing` (repo root, gitignored `.gitignore:5`) | `load-signing-env.sh:4-9`, sourced by every package script | local defaults for all of the above; **absent on this Mac** |

Tauri's own env names (`APPLE_SIGNING_IDENTITY`, `APPLE_CERTIFICATE[_PASSWORD]`, `APPLE_ID`/`APPLE_PASSWORD`/
`APPLE_TEAM_ID` — tauri-cli-v2.11.4 `crates/tauri-cli/src/interface/rust.rs` and `tauri-bundler/src/bundle/macos/sign.rs:19-43,96-158`)
are **not** used by theuth's pipeline.

### 2.5 Notarization and the gate

- `scripts/notarize-macos.sh:38-42`: `xcrun notarytool submit <dmg> --wait`, `xcrun stapler staple <dmg>`,
  `spctl --assess --type open --context context:primary-signature -vv <dmg>`. Only the **DMG** is submitted; the
  `.app` inside is covered by the DMG ticket and `package-update-macos.sh:51` tars the same `.app` for the updater.
- `scripts/release-s3.sh:188-198` gates: `codesign -d --verbose=2` must show `Authority=Developer ID` (app and helper),
  gate 3 `spctl --assess --type execute "$APP"` must pass ("not notarized"), unless `THEUTH_RELEASE_PRERELEASE=1`
  (`:64`). `docs/design/39-update.md:27,599-602` documents the same gate.

### 2.6 Facts that matter for the companion (`Contents/Library/LoginItems/Theuth Companion.app`)

1. tauri-bundler copies arbitrary `bundle.macOS.files` entries into `Contents/<path>` (tauri-cli-v2.11.4
   `crates/tauri-bundler/src/bundle/macos/app.rs:182-207`, `copy_custom_files_to_bundle`), but those files are **not
   added to `sign_paths`** (`app.rs:77-113` collects only frameworks, external binaries and the main binary) — so a
   nested helper `.app` copied that way would reach the signing step unsigned even on the bundler path. Irrelevant for
   theuth today, because `package-macos.sh` signs by hand; the companion must simply be added to the inside-out list
   **before** `:125` (the outer `.app` signature) — see §4.
2. The bundler passes **one entitlements file to every target** (`sign.rs:53-70` loops `targets` with the same
   `settings.macos().entitlements`); `package-macos.sh` already deviates (helper plist vs app plist), which is the
   right shape for a companion that needs different entitlements from the main app.
3. `codesign --verify --deep --strict` at `package-macos.sh:127` will also verify the nested companion, so an unsigned
   or mis-signed helper fails the package step locally, before notarization.

## 3 Time-sensitive verdict

**Verdict: YES — a Developer-ID-signed, notarized, non-App-Store macOS app may carry
`com.apple.developer.usernotifications.time-sensitive`, PROVIDED the bundle that claims it embeds a Developer ID
provisioning profile (`Contents/embedded.provisionprofile`) whose App ID has the "Time Sensitive Notifications"
capability enabled. Tauri's bundler does not generate or embed such a profile; it can only copy one you supply.**

Evidence, in order:

1. **Apple account help, "Supported capabilities (macOS)"**
   (https://developer.apple.com/help/account/reference/supported-capabilities-macos/, fetched 2026-10-07 12:00, raw HTML
   saved at `/tmp/130-spikes/SC/supported-capabilities-macos.html`): the page opens with "The capabilities available to a
   macOS provisioning profile depend on your program membership and signing certificate" and its table has three columns
   — ADP, **Developer ID**, Apple Developer (free). Row **"Time Sensitive Notifications": ADP ✓, Developer ID ✓, Apple
   Developer —** (parsed from the `<figure class="icon icon-checksolid" alt="yes">` cells; the text rendering drops the
   icons, hence the raw-HTML parse). For comparison, in the same table: Push notifications ✓/✓/—, Communication
   Notifications ✓/✓/—, App groups ✓/✓/✓, Hardened runtime ✓/✓/✓, Keychain sharing ✓/✓/✓; Developer-ID-NOT-supported
   rows are Game Center, HLS Interstitial Previews, HomeKit, In-App Purchase, Low Latency HLS, Managed App
   Installation UI, Messages Collaboration, Sensitive Content Analysis, Shared with You, Sign in with Apple, WeatherKit.
2. **The entitlement key exists and is the designated mechanism.** The local SDK header
   `/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/System/Library/Frameworks/UserNotifications.framework/Headers/UNUserNotificationCenter.h:31`
   reads `UNAuthorizationOptionTimeSensitive API_DEPRECATED("Use time-sensitive entitlement", … macos(12.0, 12.0) …)`;
   Apple's doc for `UNAuthorizationOptions.timeSensitive`
   (https://developer.apple.com/documentation/usernotifications/unauthorizationoptions/timesensitive) carries the same
   deprecation summary "Use time-sensitive entitlement" (introduced and deprecated macOS 12.0). `UNNotificationContent.h:28`
   has `UNNotificationInterruptionLevelTimeSensitive`; `UNNotificationSettings.h:80` has `timeSensitiveSetting`
   (macOS 12+). The exact key string is checked by the notification daemon on this Mac: `strings /usr/sbin/usernoted`
   (macOS 26.6.2 build, 2026-08-13) lists `com.apple.developer.usernotifications.time-sensitive` next to
   `…usernotifications.communication`, `…critical-alerts`, `…filtering`. Microsoft's MAUI Mac Catalyst entitlements page
   (https://learn.microsoft.com/en-us/dotnet/maui/mac-catalyst/entitlements) spells the same key, type Boolean.
   Apple's own entitlement reference page for this key is **404** at both
   `/documentation/bundleresources/entitlements/com.apple.developer.usernotifications.time-sensitive` and its
   `/tutorials/data/…json` twin; the Entitlements index (read in full, 64 KB JSON) lists only `…critical-alerts` and
   `…filtering` under User Notifications — so Apple documents the capability in Account Help and Xcode, not in the
   entitlement reference.
3. **A provisioning profile IS required** — TN3125 "Inside Code Signing: Provisioning Profiles"
   (https://developer.apple.com/documentation/technotes/tn3125-inside-code-signing-provisioning-profiles, §"Entitlements
   on macOS"): "A macOS app can claim certain entitlements without them being authorized by a provisioning profile. These
   *unrestricted entitlements* include: those used to enable and configure the App Sandbox, those used to configure the
   Hardened Runtime, [and `com.apple.security.application-groups`]. In contrast, *restricted entitlements* must be
   authorized by a provisioning profile. … A Mac app that uses no restricted entitlements doesn't need a provisioning
   profile. … macOS supports provisioning profiles for both App Store and Developer ID distribution. Some entitlements
   are not supported by Developer ID profiles. For the details, see Supported capabilities (macOS)." and §"Profile
   location": "macOS expects to find the profile at `MyApp.app/Contents/embedded.provisionprofile`". The same technote
   adds: "A standalone executable can't claim a restricted entitlement because there's no place to embed the provisioning
   profile … wrap it in an app-like structure" — which is exactly why the companion must be a `.app`, not a bare helper
   binary, if it is the process that posts time-sensitive notifications. `com.apple.developer.usernotifications.time-sensitive`
   is not in the unrestricted list ⇒ restricted ⇒ profile required. (Also: Developer ID profiles "have an expiration date
   far in the future", TN3125 §"The when".)
4. **Managed / approval status: UNVERIFIED.** Account Help "Provisioning with capabilities"
   (https://developer.apple.com/help/account/reference/provisioning-with-managed-capabilities, saved at
   `/tmp/130-spikes/SC/provisioning-with-managed-capabilities.html`) says "Managed capabilities require approval from
   Apple to use … Once enabled, new provisioning profiles for that App ID will automatically include the associated
   entitlements", but neither it nor "Capability requests" enumerates which capabilities are managed; Time Sensitive
   Notifications appears in the public capabilities table like Push Notifications. Expect it to be a toggle in
   Certificates, Identifiers & Profiles → Identifiers → <App ID> → Capabilities; if the toggle is absent there, it is
   managed and needs a request (manual step, not run).
5. **Tauri's bundler and `embedded.provisionprofile`** (tauri-cli 2.11.4 is what is installed — `~/.cargo/.crates.toml`;
   its `crates/tauri-cli/Cargo.toml` pins `tauri-bundler = "2.9.4"`; source read at tag `tauri-cli-v2.11.4` on GitHub
   because `~/.cargo/registry/src` holds no bundler crate):
   - The only mention of `provisionprofile` in the whole tauri repo (GitHub code search, 2 hits) is a doc comment in
     `crates/tauri-utils/src/config.rs:1490-1491` (local copy `~/.cargo/registry/src/index.crates.io-*/tauri-utils-2.9.3/src/config.rs:1490-1491`):
     "Note that universal app links require signed apps with a provisioning profile to work. You can accomplish that by
     including the `embedded.provisionprofile` file in the `macOS > files` option." The second hit is a test fixture's
     `CodeResources` rule `^embedded\.provisionprofile$` (weight 20 — the standard codesign resource rule).
   - `crates/tauri-bundler/src/bundle/macos/app.rs:182-207` `copy_custom_files_to_bundle` copies each `bundle.macOS.files`
     entry to `Contents/<key>`; nothing in the bundler reads, validates or generates a profile. So `"files": {
     "embedded.provisionprofile": "./Theuth.provisionprofile" }` in a tauri conf would place the MAIN app's profile; the
     companion's profile must be inside the companion bundle (`Contents/Library/LoginItems/Theuth Companion.app/Contents/embedded.provisionprofile`),
     i.e. put there by whatever builds the companion, before the inside-out signing in `package-macos.sh`.
   - `crates/tauri-macos-sign/src/provisioning_profile.rs` (`ProvisioningProfile::from_base64`, writes
     `<uuid>.mobileprovision`) is used only by `crates/tauri-cli/src/mobile/ios/mod.rs` — iOS, not the macOS bundler.
   - `crates/tauri-macos-sign/src/keychain.rs:204-245` is the codesign call: `codesign --force -s <identity>
     [--options runtime] [--keychain p] [--entitlements f] <path>` — no `--deep`, no `--timestamp` flag (the bundler
     relies on codesign's default, which adds a timestamp only when signing with a real identity).

Consequence for the design: the companion needs (a) its own App ID (e.g. `io.theuth.mac.companion`) with Time Sensitive
Notifications enabled, (b) a **Developer ID Application** provisioning profile for that App ID downloaded from the
developer account and copied to `Theuth Companion.app/Contents/embedded.provisionprofile`, (c) the entitlement in the
companion's entitlements plist together with `com.apple.application-identifier` = `<TEAMID>.io.theuth.mac.companion` and
`com.apple.developer.team-identifier` = `<TEAMID>` (both are in every profile's allowlist; TN3125 §"The what"), and (d)
signing with the Developer ID certificate on the release Mac — an ad-hoc build on this Mac cannot exercise any of it
(the entitlement is unauthorized without a matching profile → the process is killed at launch; manual verification is
`codesign -d --entitlements - --xml <app>` + `security cms -D -i <app>/Contents/embedded.provisionprofile`).

## 4 Nested signing requirements

What a nested `Contents/Library/LoginItems/Theuth Companion.app` must satisfy, with Apple's words:

| Requirement | Source |
|---|---|
| **Signed with a Developer ID Application certificate, not ad hoc** — "Use a 'Developer ID' application … certificate for your code-signing signature. (Don't use a Mac Distribution, ad hoc, Apple Developer, or local development certificate.)" and "Enable code-signing for **all of the executables** you distribute" | "Notarizing macOS software before distribution", https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution (§Prepare your software for notarization) |
| **Hardened runtime on every executable** — "Enable the Hardened Runtime capability for your app and command line targets" | same page |
| **Secure timestamp** — "Include a secure timestamp with your code-signing signature. (The Xcode distribution workflow includes a secure timestamp by default. For custom workflows, [pass `--timestamp`])"; "Generating a secure timestamp requires internet access. macOS accepts only one secure timestamp server" | same page; "Resolving common notarization issues" https://developer.apple.com/documentation/security/resolving-common-notarization-issues (§Include a secure timestamp) |
| **No `get-task-allow`**, link against macOS 10.9+ SDK, XML ASCII entitlements | "Notarizing…" (same list) |
| **Inside-out order** — "signatures record nested code by its code signature and embed that information in the (outer) signature's resource envelope, recursively. This means that when a code signature is created, all nested code must already be signed correctly or the signing attempt will fail. … build and sign targets inside out: build the innermost code, copy it into the next-outer bundle, then sign that, etc., signing the outermost bundle last." | TN2206 "macOS Code Signing In Depth" §Nested Code, https://developer.apple.com/library/archive/technotes/tn2206/_index.html |
| **`Contents/Library/LoginItems` is a sanctioned nested-code location** — TN2206 Table 3 "Standard locations for code inside a bundle": `Contents/Library/LoginItems — Installable login items` (also `Contents/MacOS` helper apps and tools, `Contents/Frameworks`, `Contents/PlugIns`, `Contents/XPCServices`, `Contents/Helpers`, `Contents/Library/LaunchServices`). "These places are expected to contain only code. Putting arbitrary data files there will cause them to be rejected." | TN2206 §Nested Code, Table 3 |
| **`--deep`** — TN2206 §"Using the codesign Tool's --deep Option Correctly": "When verifying signatures, add `--deep` to perform recursive validation of nested code … Note that Gatekeeper always performs `--deep` style validation. Note that signing with the combination `--deep --force` will forcibly re-sign all code in a bundle." i.e. Apple's archive positions `--deep` as a *verification* flag; the current notarization-issues page likewise uses `--deep` only in `codesign --verify --deep --strict --verbose=2`. Signing with `--deep` would also stamp ONE entitlements file onto every nested executable — fatal here, because the companion's entitlements (time-sensitive + application-identifier) must differ from the main app's. The explicit "don't sign with --deep, sign inside-out" text that tauri-bundler itself cites (`app.rs:120-121` → https://developer.apple.com/forums/thread/701514, Quinn's "Signing a Mac Product For Distribution") is behind a bot-verification wall from this machine — **wording unverified**, the practice is corroborated by TN2206 and by the bundler's own comment. | TN2206; tauri-bundler `app.rs:120-121`; forum thread 701514 (unverified) |
| **Same Team ID** — for Service Management helpers Apple states: "The Team Identifier of the LaunchAgent or LaunchDaemon executable in the legacy property list must match that of the app bundle for the AssociatedBundleIdentifiers key" and, for the new bundle structure, "a `LaunchAgent` and `LaunchDaemon` in the app bundle automatically associate with that app in the Login Items panel"; the hardened runtime's library validation only loads code "signed by Apple or by the same Team ID" (dylibs/frameworks). A sentence saying literally "a LoginItems helper must share the Team ID" was not found in the pages read — **unverified as a literal rule**, but with one Developer ID certificate on the release Mac it holds by construction. | "Updating helper executables from earlier versions of macOS", https://developer.apple.com/documentation/servicemanagement/updating-helper-executables-from-earlier-versions-of-macos; Hardened Runtime docs (not re-read here) |
| **`SMAppService.loginItem(identifier:)`** — "The bundle identifier of the helper application … The property list name must correspond to a property list in the calling app's `Contents/Library/LoginItems` directory" (macOS 13+). | https://developer.apple.com/documentation/servicemanagement/smappservice/loginitem(identifier:) |

Where this lands in theuth's pipeline (§2): insert, between `package-macos.sh:124` (sidecar) and `:125` (outer app),
`codesign --force --options runtime --timestamp --sign "$IDENTITY" --entitlements companion.entitlements
"$APP/Contents/Library/LoginItems/Theuth Companion.app"` — after signing anything nested inside the companion first
(its own frameworks/XPC services, if any). `codesign --verify --deep --strict` at `:127` then validates it, and
`notarize-macos.sh` needs no change (the DMG submission covers nested code; the notary log will list the companion as a
separate "path" entry and must show no issues for it).

## 5 Extensions without Xcode

**Verdict: NO documented non-Xcode path. Apple documents app extensions (incl. WidgetKit extensions) only as Xcode
targets; SwiftPM has no product type that yields a `.appex`. A hand-rolled `.appex` is not documented anywhere Apple
publishes — UNVERIFIED/undocumented, treat as out of scope while this Mac has Command Line Tools only.**

- Apple, "Creating a widget extension" (https://developer.apple.com/documentation/widgetkit/creating-a-widget-extension,
  §"Add a widget target to your app"): "The Widget Extension template provides a starting point for creating your widget.
  The template creates an extension target that contains a single widget. … Open your app project in Xcode and choose
  File > New > Target. From the Application Extension group, select Widget Extension, and then click Next." Later sections
  are "Preview widgets in Xcode" and "enable the Data Protection capability for your widget extension in Xcode".
- Apple, App Extension Programming Guide → "Creating an App Extension"
  (https://developer.apple.com/library/archive/documentation/General/Conceptual/ExtensibilityPG/ExtensionCreation.html):
  "Use the corresponding Xcode app extension template and enhance the default files with custom code and user interface"
  and (§Examine the Default App Extension Template) each template ships an `Info.plist` with the `NSExtension` /
  `NSExtensionPointIdentifier` keys that the extension point requires — the bundle format is only described through
  the templates.
- SwiftPM `Product` (https://developer.apple.com/documentation/packagedescription/product): "A package product can be one
  of three types: library product … executable product … plugin product." No extension/bundle product. Confirmed
  locally: `swift package init --help` (Swift 6.3.2, run 2026-10-07) lists `--type library | executable | tool |
  build-tool-plugin | command-plugin | macro | empty` — nothing that produces an `.appex` or any bundle.
- Manual path: in principle an `.appex` is a bundle with `NSExtension` keys and a Mach-O built with `-application-extension`
  (`swiftc -application-extension` / `ld -application_extension`), signed as nested code under `Contents/PlugIns/`
  (TN2206 Table 3). No Apple page documents assembling one outside Xcode, Xcode's `pluginkit` registration and the
  WidgetKit-specific `Info.plist` keys (`NSExtensionPointIdentifier = com.apple.widgetkit-extension`) are only visible in
  the template — **unverified; do not plan on it**. Also note the WidgetKit page: "iOS widgets aren't available as iPhone
  widgets on Mac", so a Mac widget needs a macOS WidgetKit extension in the Mac app.
- `xcodebuild` is unavailable here (`xcode-select -p` = /Library/Developer/CommandLineTools); `scripts/package-ios.sh:36-40`
  already fails with "full Xcode required" on this machine — the WidgetKit/App Intents extension work would have to run on
  the Xcode-equipped release Mac, like the iOS build does.

**WidgetKit refresh budget** — "Keeping a widget up to date"
(https://developer.apple.com/documentation/widgetkit/keeping-a-widget-up-to-date, §"Plan reloads within a budget"),
verbatim: "WidgetKit uses a budget to distribute widget reloads over the course of the day. … WidgetKit maintains
different budgets for each active widget the user adds to their device. … A widget's budget applies to a 24-hour period.
… **For a widget the user frequently views, a daily budget typically includes from 40 to 70 refreshes. This rate roughly
translates to widget reloads every 15 to 60 minutes**, but it's common for these intervals to vary … The system takes a
few days to learn the user's behavior. During this learning period, your widget may receive more reloads than normal."

Does `WidgetCenter.shared.reloadTimelines(ofKind:)` count? **Yes, except in the listed exemptions.** Same page,
§"Inform WidgetKit when a timeline changes": "Your app can tell WidgetKit to request a new timeline … To reload a specific
type of widget, your app uses `WidgetCenter.shared.reloadTimelines(ofKind:)` … you can use `reloadAllTimelines()` to
reload the timelines for all your widgets" — these are reloads, and the budget section says "limit the frequency and
number of updates you request to what's necessary". Then: "Cases in which WidgetKit doesn't count reloads against your
widget's budget include when: The widget's containing app is in the foreground. The widget's containing app has an
active audio or navigation session. The widget's containing app is the current Now Playing app … The widget performs an
app intent, such as when the user taps a button or toggles a switch. The widget performs an animation. The system locale
changes. Dynamic Type or Accessibility settings change." The page does not contain a sentence saying literally
"`reloadTimelines` counts against the budget"; the inference is from the budget applying to all reloads minus the
exemption list (an "app in the foreground" exemption would be meaningless otherwise). Also offered on that page:
WidgetKit push notifications (APNs) as "an additional way to update your widget that doesn't replace timeline updates".

## 6 macOS version note

- This Mac: `sw_vers` → `ProductVersion 26.6.2`, `BuildVersion 25G83` (macOS Tahoe 26). SDK in Command Line Tools: 26.5.
- Apple Support "Find out which macOS your Mac is using" (https://support.apple.com/en-us/109033, Published Date:
  September 28, 2026, read rendered 2026-10-07 12:20), table "Which macOS is the latest?": **macOS 27 Golden Gate —
  27.0.1**; macOS Tahoe 26 — 26.7.1; macOS Sequoia 15 — 15.8.1; macOS Sonoma 14 — 14.8.9; Ventura 13 — 13.7.8.
  Compatibility page for Golden Gate: https://support.apple.com/en-us/127255.
- Apple developer release-notes index (https://developer.apple.com/documentation/macos-release-notes) lists "macOS Tahoe
  26 … 26.6 Release Notes", "macOS 27 Golden Gate Release Notes" and "macOS 27.2 Golden Gate Beta 3 Release Notes" — so
  27.0.x is shipping and 27.2 is in beta.
- Corroborated locally: `softwareupdate --list` (run 2026-10-07 12:06, read-only) offers "macOS Tahoe 26.7.1-25G241
  (Action: restart)", "Command Line Tools for Xcode 27.0" and "Command Line Tools for Xcode 26.6" — i.e. this Mac is one
  Tahoe point release behind and Xcode 27 tooling is already published for it. Nothing was installed.
- Design implication: theuth's `tauri.macos.conf.json:41` sets `minimumSystemVersion: "15.0"` (Sequoia). Every API this
  design leans on is older than that floor: `UNNotificationInterruptionLevel.timeSensitive`, the time-sensitive
  entitlement and `timeSensitiveSetting` are macOS 12+ (SDK headers above); `SMAppService.loginItem` and the
  `Contents/Library/LoginItems` bundle structure are macOS 13+; `NSStatusItem.expandedInterfaceDelegate` is macOS 27-only
  (per the companion-surface recon) and must be `#available`-gated. State in the design: "APIs assumed as of macOS 26 SDK
  (26.5 on the build Mac); floor 15.0 inherited from the Tauri app; verified on 26.6.2; macOS 27.0.1 is current but not
  tested here."

## Manual steps left for the user

1. On the release Mac (the one with the Developer ID certificate, team `8H548FVL95` per `scripts/package-ios.sh:33`):
   Certificates, Identifiers & Profiles → Identifiers → register `io.theuth.mac.companion` (explicit App ID) → enable
   **Time Sensitive Notifications** (if the toggle is missing, it is a managed capability → "Request" it from the
   Account Holder — §3 item 4) → Profiles → + → **Developer ID** (macOS) profile for that App ID → download → commit as
   `crates/desktop/companion/Theuth_Companion.provisionprofile` (profiles contain no private material) → the companion
   build copies it to `Theuth Companion.app/Contents/embedded.provisionprofile`. Verify with
   `security cms -D -i <profile> | plutil -p -` that `Entitlements` contains
   `com.apple.developer.usernotifications.time-sensitive = 1` and `ProvisionsAllDevices = 1`.
2. First Developer-ID build with the companion: `THEUTH_SIGN_IDENTITY="Developer ID Application: …" just package-macos`,
   then `codesign -d --entitlements - --xml "…/Theuth Companion.app"` and launch the helper once to confirm macOS does not
   kill it (`log stream --predicate 'subsystem == "com.apple.securityd" OR process == "amfid"'` shows the reason if it does).
3. `just notarize-macos` → in the notary log (`xcrun notarytool log <id>`) confirm the companion appears with no issues.
4. Not run here (would prompt): `UNUserNotificationCenter.requestAuthorization`, the Login Items approval in System
   Settings, and `tccutil`. None was executed by this spike.

## Surprises

- This Mac has **zero** code-signing identities of any kind — every local `package-macos` build is ad-hoc by
  construction; the Developer ID material lives only on the release Mac.
- There is no CI at all (`.github/` absent); the whole release is `just` recipes run by hand, and `release-s3.sh:133`
  hard-refuses to run anywhere but an arm64 Mac.
- Tauri's bundler collects sign targets from frameworks/binaries only (`app.rs:77-113`); anything placed via
  `bundle.macOS.files` is copied but never signed — irrelevant today because `package-macos.sh` signs by hand, but a
  trap if anyone moves the companion into the tauri config instead of the script.
- Apple has **no entitlement reference page** for `com.apple.developer.usernotifications.time-sensitive` (404) and the
  Entitlements index omits it, while the SDK header deprecates the API option in favour of "the time-sensitive
  entitlement" and `/usr/sbin/usernoted` checks the exact string — the capability is documented only in Account Help.
- Apple's search-facing docs are JS shells; the reliable reads were the `/tutorials/data/…json` twins, raw HTML via
  `http_request` for Account Help, and the Documentation Archive for TN2206. developer.apple.com/forums is behind a
  bot-verification wall from this network (the `--deep` thread tauri cites could not be read).
- Apple's support page (28 Sep 2026) puts the current macOS at 27.0.1 (Golden Gate); this Mac is a Tahoe point release
  behind (26.6.2 vs 26.7.1).
- Runtime trap for the orchestrator: names bound by tuple-unpacking an `await asyncio.gather(...)` did not persist to
  the next cell (plain assignments in the same cell did) — rebind each result on its own line.

