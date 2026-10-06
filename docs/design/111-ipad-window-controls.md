# 111 – iPadOS 26 window controls over the top-left chrome

Status: fix · 2026-10-06 · branch `fix/ipad-window-controls`, base `5044521e`. Amends 31 §8 (the
iOS lifecycle hooks). Not run on a device yet — §6 is Sky's checklist.

## 1. Bug

iPad, release 2.9.0: "The Mesh Selector in the top left corner of the window is behind the traffic
light buttons." iPadOS 26 windowing draws macOS-style controls over the top-left of every windowed
scene. For a Tauri app they float over the webview, add nothing to the safe area
(`env(safe-area-inset-top)` stays 0) and vanish in full screen, so no static CSS inset is right
(tauri-apps/tauri#15521).

## 2. Mechanism

UIKit asks the scene's `UIWindowSceneDelegate` for `preferredWindowingControlStyle(for:)`. `minimal`
moves the controls into a strip above the content and the strip counts toward the safe area; our
`--safe-top` then does the rest. Two things stand in the way on our pinned stack:

- tao 0.35.3 (`Cargo.lock:20843`) never answers the question. tao#1250 (tao 0.36, 2026-07-29) does,
  but tauri-runtime-wry 2.11.4 pins `tao ^0.35` and no tauri 2.x release moves off it (only
  3.0.0-alpha). `cargo update -p tao` has nothing to offer; `[patch.crates-io]` cannot swap a 0.37
  into a `^0.35` slot without a version-renamed fork.
- Even upstream's fix needs the scene lifecycle, which the app did not have: no
  `UIApplicationSceneManifest` in `Info.ios.plist`, so UIKit ran the legacy app-delegate lifecycle
  where there is no scene delegate to ask. tao 0.35 wires its `TaoSceneDelegate` only when the
  manifest says `UIApplicationSupportsMultipleScenes = true` (tao#1308), and when it does, its
  `application:configurationForConnectingSceneSession:options:` returns a `UISceneConfiguration` it
  has already released — release builds crash at launch (tao#1244, fixed in 0.36 only).

## 3. Option taken: (b), the runtime patch

Option (a) is impossible (§2); (c) was rejected as a fork with unknown wry/tauri-runtime-wry
compatibility. (b) in `crates/desktop/src/ios_windowing.rs`, wired from `lib.rs::run` between
`Builder::build` (tao registers `AppDelegate` in `EventLoop::new`) and `App::run`
(`UIApplicationMain` instantiates it):

1. `Info.ios.plist` (and the committed `gen/apple/desktop_iOS/Info.plist`) declare
   `UIApplicationSceneManifest { UIApplicationSupportsMultipleScenes = true, UISceneConfigurations = {} }`
   — exactly the shape tao's scene path expects (and the one tao#1244 was reproduced with).
2. `install()` replaces `AppDelegate`'s `application:configurationForConnectingSceneSession:options:`
   with ours. Ours calls tao's first (side effect: it registers `TaoSceneDelegate`; the dangling
   pointer it returns is discarded), then `class_addMethod`s `preferredWindowingControlStyleForScene:`
   → `UISceneWindowingControlStyle.minimalStyle` onto `TaoSceneDelegate` (plus the
   `UIWindowSceneDelegate` protocol), and returns a fresh, autoreleased configuration pointing at that
   class. Idempotent per connection; no-op when the manifest (and so tao's method) is absent.
   `UISceneWindowingControlStyle` is only touched inside the callback UIKit ≥ 26 sends, so older iOS
   never resolves the class.
3. Lifecycle consequences handled in `lib.rs::mobile::on_event`:
   - tao 0.35's scene delegate emits no `Suspended`/`Resumed`; it reports `sceneWillResignActive` /
     `sceneDidBecomeActive` as `Focused(false/true)`. The 31 §8 background task now keys on both
     pairs (`hold` / `release` are idempotent, so the legacy iPhone path is unchanged).
   - `UIApplicationSupportsMultipleScenes = true` lets iPad users ask for a second window. tao reports
     it as `RunEvent::SceneRequested`; we request the session's destruction on the next run-loop turn
     (`ios_windowing::dismiss`) — the app is one window. ponytail: build a `WebviewWindow` for the
     scene instead once the UI supports more than one.

Deep links: in scene mode tao forwards `scene:openURLContexts:` and `scene:continueUserActivity:`
as `Event::Opened`, same as before. Cold-start URLs (app not running) arrive in the connection
options, which tao 0.35 does not read — §6 asks for that check.

## 4. UI safe-area audit

Everything top-left already honours `--safe-top` on iOS: `routes/__root.tsx:61` pads the routed
column (`SidebarInset` → chat `Header`, `SectionBand`, Attention/Library bands, Hud), and the
sidebar pads itself in both forms (`components/ui/sidebar.tsx:237` desktop, `:180` Sheet); the mesh
selector (`EngineSwitcher`, in `AppSidebar.tsx:183`'s `SidebarHeader`) sits inside the padded
sidebar. The sidebar is `collapsible="offcanvas"`, so there is no rail to pad. The `pl-20`
traffic-light inset stays mac-only (`isMacDesktop()`). One gap closed: the sidebar had
`--safe-top`/`--safe-bottom` but not `--safe-left`, unlike `SidebarInset`; added
`pl-[var(--safe-left)]` at `sidebar.tsx:180,237` (iPhone landscape; 0 on iPad).

## 5. Fallback decision

No static iPad inset was added. With `minimal` the system reports the strip through the safe area,
so the existing `--safe-top` plumbing is the fix. If §6 shows the controls still overlapping, the
next step is to debug the scene path (Console: does `TaoSceneDelegate` get `preferredWindowingControlStyleForScene:`?),
not a `deviceName() === 'iPad'` inset.

## 6. Sky verifies on device (`just release-ios`)

1. Launch (release build) on iPad 26 and iPhone: no crash at launch (the tao#1244 path is ours now).
2. iPad, both window modes. Sky's observation on 2.9.0: the overlap happens only when the window is
   not full screen — in full screen the controls are gone (tauri#15521), which is why no static inset
   is right. Expected now: windowed → controls in their own strip, `--safe-top` grows, sidebar header /
   mesh selector and the first-pane header sit below it; full screen → no strip, chrome at the top as
   today. Toggle between the two with the app open; the chrome must follow without a reload.
3. Background the app mid-turn → notification still arrives (background task via `Focused`).
4. Deep link: scan a pairing QR / open a universal link with the app running and cold.
5. iPad "New Window" (window menu or App Exposé "+"): the extra window closes itself.

## 7. Files

`crates/desktop/src/ios_windowing.rs` (new), `crates/desktop/src/lib.rs`, `crates/desktop/Cargo.toml`
(objc2-ui-kit scene features, `objc2-foundation` on iOS), `crates/desktop/ios/Info.ios.plist`,
`crates/desktop/gen/apple/desktop_iOS/Info.plist`, `crates/desktop/ui/src/components/ui/sidebar.tsx`.

## 8. Gates

- `cargo check -p desktop` (macOS host): green.
- `cargo clippy --no-deps -p desktop --all-targets -- -D warnings`: clean.
- iOS: `rustup target add aarch64-apple-ios` ok (std installed for 1.98.1); `xcrun --sdk iphoneos
  --show-sdk-path` → `SDK "iphoneos" cannot be located` (no Xcode on this host, CLT only), so
  `cargo check --target aarch64-apple-ios -p desktop` stops at `objc2-exception-helper`'s build
  script. Substitute: `ios_windowing.rs` was compiled and clippy-checked for `aarch64-apple-ios` in
  a throwaway crate with the same objc2 0.6 / objc2-foundation 0.3 / objc2-ui-kit 0.3 feature set and
  a stubbed `tauri::AppHandle` — clean. The `lib.rs` mobile arms are `#[cfg(mobile)]` and unchecked
  here.
- UI (`crates/desktop/ui`): `bun run check` green, `CI=1 bun run test` 142 files / 825 tests passed,
  `bun run build` ok. Bindings: the copy from Sky's checkout (2026-10-04) predates `AttentionItem.packet`
  / `ask` and `Item.ts` in this tree and failed `tsc`, so they were regenerated here with the body of
  `just ts` (`cargo test -p rpc --features ts --lib export_bindings`, own target dir, gitignored output)
  and copied as `just ts-desktop` does.
