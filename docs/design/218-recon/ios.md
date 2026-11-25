# Recon report: design 218 Whiteboard, native iPad parts

Nothing in the repo was modified. This session had no `shell`, so I could not run git, build or boot a simulator. Everything below comes from reading files and Apple's docs JSON. `[unverified]` marks a claim I could not confirm.

## 0. Design read (§6 and what depends on it)

- **Native capture is capture-only.** `docs/design/218-whiteboard.md#L274-L283`: a transparent `PKCanvasView` sits over the main webview, never transformed, with `drawingPolicy = .pencilOnly`. Its `hitTest` returns nil, and its `drawingGestureRecognizer` is moved onto the web view with `allowedTouchTypes = [.pencil]`.
- **Hand-over.** At pen-idle (120 ms) the plugin samples new strokes with `interpolatedPoints(in:by:)`, converts screen to world coordinates, and sends them to JS. JS acks after painting. No ack within 300 ms means the stroke stays and the hand-over retries.
- **Picker and responder.** The picker is `PKToolPicker(toolItems:)` (iOS 18+) with monoline, pen, marker and eraser items. It is attached through the web view's `pencilKitResponderState.activeToolPicker`, which requires iPadOS 26. The web view stays first responder.
- **Pencil taps.** A `UIPencilInteraction` on the web view follows the user's preferred tap and squeeze actions (`#L282`).
- **Snapshots.** `takeSnapshot(rect)` of the main webview, via a new `wb_capture(rect)` command added in P0 (`#L125-L131`). `describe-point` goes over the page bridge (`#L298`). Vision handwriting runs per mark image (`#L302`).
- **Plugin independence.** There is no `tauri-plugin-glass` on main and no 198 design doc. The ink plugin's build wiring is done fresh (`#L332-L334`). P3 is committed regardless of the productivity test (R11, `#L387`). The exit gate is at `#L283`.
- **Unverified in the design itself** (`#L429`): snapshot of sandboxed frames, the recogniser routing over WKWebView, and `hitTest` behaviour. These are P0 and P3 spikes.

## 1. Tauri mobile plugins with Swift in this repo

**Not available here:**
- `crates/` on this worktree has no `tauri-plugin-*` directory, and no `docs/design/198*`.
- The worktree's `.git` is a file pointing at `/Users/sky/Develop/skymoore/theuth/.git`. That repo's `packed-refs#L42-L47` lists six unmerged branches: `origin/feat/198-glass-composer`, `-glass-controls`, `-glass-everywhere`, `-glass-ios`, `-glass-mac` and `-liquid-glass`. Two `backup/theuth-198c` and `backup/theuth-198d` refs are at `#L2-L3`.
- I found no on-disk checkout of the glass plugin under `~/Develop` or `~/Develop/skymoore`.
- Knowledge records the glass plugin's shape only: `crates/tauri-plugin-glass/ios/*.swift`, `ios/Checks/run.sh` (a Mac Catalyst check harness), and `init_plugin_glass` linked via swift-rs.

**Reference pattern.** The official `tauri-plugin-haptics` 2.4.2 is in the cargo registry at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tauri-plugin-haptics-2.4.2/`. The repo already uses it (`Cargo.lock#L22560`, `crates/desktop/Cargo.toml#L66`, `lib.rs#L107-L111`):
- **`Cargo.toml`.** `links = "tauri-plugin-haptics"`, `tauri = { version = "2.12", default-features = false }`, `[build-dependencies] tauri-plugin = { version = "2.7.1", features = ["build"] }`. For iOS it adds `tauri` with `features = ["wry"]`.
- **`build.rs`.** `tauri_plugin::Builder::new(COMMANDS).global_api_script_path("./api-iife.js").android_path("android").ios_path("ios").try_build()`. `COMMANDS` is snake_case.
- **iOS wiring (`tauri-plugin-2.7.2/src/build/mobile.rs#L139-L160`).** The build script copies the Tauri Swift API into `ios/../.tauri/tauri-api` and calls `tauri_utils::build::link_apple_library(pkg, ios_path)`. So `gen/apple/project.yml` needs no edit. It only links `libapp.a` and runs `cargo tauri ios xcode-script` (`project.yml#L74-L89`).
- **`ios/Package.swift`.** Tools version 5.3, `.iOS(.v13)`, a static library, and dependency `.package(name: "Tauri", path: "../.tauri/tauri-api")`. The target is `path: "Sources"`.
- **Swift class.** `class HapticsPlugin: Plugin` with `@objc public func cmd(_ invoke: Invoke) throws`. Args come from `invoke.parseArgs(T.self)`, and you finish with `invoke.resolve()` or `reject`. The file ends with `@_cdecl("init_plugin_haptics") public func initPlugin() -> Plugin`.
- **Rust.** `src/lib.rs#L46-L64`: `Builder::new("haptics").invoke_handler(...).setup(|app, api| { #[cfg(mobile)] mobile::init(app, api)?; ... app.manage(h) })`. `src/mobile.rs#L16-L17`: `tauri::ios_plugin_binding!(init_plugin_haptics);` plus `api.register_ios_plugin(init_plugin_haptics)` (the register call is truncated in my read). Commands are `#[command] async fn x<R: Runtime>(app: AppHandle<R>, …)` calling `self.0.run_mobile_plugin("camelName", payload)`.
- **Swift ⇄ Rust boundary (`.tauri/tauri-api/Sources/Tauri`).** `Tauri.swift#L120-L156` has `register_plugin`, `on_webview_created(webview, viewController)` and `run_plugin_command`. Commands run on `ipcDispatchQueue`, a background queue (`Tauri.swift#L61`), so hop to main for UIKit. `Plugin.swift#L33` is `@objc open func load(webview: WKWebView)`. `Plugin.swift#L43-L57` is `trigger(event, data:)`, which sends to channels registered via `registerListener`/`removeListener` (`#L59-L81`). `Channel.send(_ Encodable)` is at `Channel.swift#L60`.
- **Main-app registration.** `crates/desktop/src/lib.rs#L107-L111` (`mobile::configure`) chains `.plugin(tauri_plugin_haptics::init())` and a mobile `invoke_handler`. The deps for iOS/Android are at `Cargo.toml#L63-L66`. `members = ["crates/*"]` (`Cargo.toml#L3`), so a new crate is picked up automatically. Plugin pins are at `Cargo.toml#L388-L396`, with tauri 2.12.5 and tauri-plugin 2.7.2 locked.
- **JS side.** `@tauri-apps/api ~2.12.4` and `@tauri-apps/plugin-haptics ~2.4.2` (`crates/desktop/ui/package.json#L34-L39`). `ui/src/lib/haptics.ts#L5-L15` dynamically imports the plugin and tolerates its absence. Copy that pattern.
- **Permissions.** `crates/desktop/capabilities/mobile.json#L6-L18` is the iOS and Android capability for window `main`. It has no custom plugin entries today, so add `"ink:default"` there. `default.json#L6` is desktop-only. For listeners, add `register_listener` and `remove_listener` to `COMMANDS`, as `tauri-plugin-notification` 2.8.0 does (`build.rs#L10-L11`).

## 2. Getting the WKWebView

- **From Swift (best for the plugin).** Tauri calls `Plugin.load(webview:)` with the main `WKWebView`. `PluginManager.shared.viewController` is the host `UIViewController`, set in `on_webview_created` (`Tauri.swift#L130-L134`). Add the `PKCanvasView` as a sibling above the web view, in `webview.superview`, with the same frame and autoresizing. Hold the web view weakly. `load` is called once per plugin, so keep references there.
- **From Rust.** `Webview::with_webview(|platform| …)` runs on the main thread, and `platform.inner()` is the `WKWebView`. Existing code:
  - `ios_webview.rs#L677-L693` (`on_main_start`): `let main = &*platform.inner().cast::<WKWebView>()`.
  - `webview.rs#L482-L492`: the same on macOS.
  - `lib.rs#L600-L612`: a `with_webview` closure on a macOS window.
- **Z-order trap.** `ios_webview.rs#L1-L5` creates web view *tile* views as raw `WKWebView` siblings placed directly above Tauri's. The overlay must sit above Tauri's view, and above the tile views if ink is to cover `web` cards. Tile pixels are not in a `takeSnapshot` of the main webview, because they are siblings (see risks).
- **Snapshot declaration.** `ios_webview.rs#L162-L170` declares `takeSnapshotWithConfiguration:completionHandler:` on a `WKWebView` class defined in that file, with the config typed as `Option<&AnyObject>`. `objc2-web-kit` 0.3 cfg-gates the typed API to macOS. The existing snapshot code is `apple_tile.rs#L270-L307` (iOS and macOS variants, always passing `None`), with a base64 PNG encoder behind it.
- **Plugin vs desktop split.** Put `wb_capture(rect)` in `crates/desktop` (extend `apple_tile.rs`), because P0 needs it on both platforms before any ink plugin exists. Keep the Swift plugin to Pencil, picker, taps and Vision.

## 3. Deployment target and gating

- **iOS minimum.** `tauri.ios.conf.json#L9-L13`: `minimumSystemVersion: "17.4"`, team `8H548FVL95`, `infoPlist: ./ios/Info.ios.plist`. `gen/apple/project.yml#L4-L6` says `deploymentTarget iOS: 17.0`. The Tauri conf wins in the build, but note the two numbers differ.
- **Info.plist.** Source is `crates/desktop/ios/Info.ios.plist#L1-L31`. The Xcode project builds from the tracked generated `gen/apple/desktop_iOS/Info.plist`. Whether the build merges new source keys is unproven, so add any new key to both. The ink plugin needs no usage-description keys.
- **Frameworks.** `project.yml#L74-L82` lists CoreGraphics, Metal, MetalKit, QuartzCore, Security, UIKit and WebKit. It does not list PencilKit or Vision. Add `linkerSettings: [.linkedFramework("PencilKit"), .linkedFramework("Vision")]` in `Package.swift` rather than editing project.yml (which `tauri ios` may regenerate). Swift autolinking from `import` may already cover it `[unverified]`.
- **Gating.** The plugin's Package.swift platform should be `.iOS(.v17)`, which is above Tauri's .v11/.v13. Gate native capture with `if #available(iOS 26.0, *)`. Squeeze APIs and `UIPencilInteraction(delegate:)` are 17.5, so also use `#available(iOS 17.5, *)`. The min is 17.4, so the 17.5 APIs are not safe without a gate even inside the iOS 26 branch.
- **Xcode.** 26.0 (17A324), SDK iphonesimulator26.0, per knowledge `facts/theuth-ios-simulator-build-on-333`. Swift 6.2.
- **`#unavailable`.** Not needed.

## 4. macOS

- **Reachable the same way.** `with_webview` gives the `WKWebView` (`webview.rs#L482-L492`, `lib.rs#L600-L612`). `Cargo.toml#L81` already enables `objc2-web-kit` features including `WKSnapshotConfiguration` on macOS, plus `WKWebView` and `block2`.
- **Existing capture.** `apple_tile.rs#L271-L289` does `takeSnapshotWithConfiguration_completionHandler(None, …)` and converts `NSImage` to a `CGImage` with `CGImageForProposedRect`.
- **For `wb_capture(rect)` on macOS:** create `WKSnapshotConfiguration`, then `setRect`, `setSnapshotWidth` and `setAfterScreenUpdates`.
- **Gap on iOS.** The `WKSnapshotConfiguration` feature is not in the iOS `objc2-web-kit` features (`Cargo.toml#L110`). Either add it and use the typed config, or keep the untyped selector and build the config with `msg_send`.
- **macOS minimum.** 15.0 (per knowledge), so no gating.

## 5. Apple API facts (developer.apple.com documentation JSON)

Platforms are the "introducedAt" values from Apple's docs JSON. Signatures marked "from memory" were not in the JSON I could parse (my tool dropped parts of it).

**PKCanvasView**
- `var drawingPolicy: PKCanvasViewDrawingPolicy { get set }`, iOS 14.0. The enum is iOS 13.0 with cases `.default`, `.anyInput`, `.pencilOnly`.
- `.pencilOnly` is documented as "Pencil touches are the only input that draw on the canvas."
- `.default` respects `prefersPencilOnlyDrawing` when the picker is visible; otherwise Pencil only.
- `allowsFingerDrawing` is deprecated ("Use drawingPolicy instead").
- `var drawingGestureRecognizer: UIGestureRecognizer { get }`, iOS 13.0, readonly.
- `var isDrawingEnabled: Bool { get set }`, iOS 18.0. Note it is on `PKCanvasView`, which matches the design.
- `var drawing: PKDrawing`, iOS 13.0.
- Delegate (iOS 13): `canvasViewDrawingDidChange(_:)` and `canvasViewDidBeginUsingTool(_:)`. `canvasViewDidEndUsingTool` and `canvasViewDidFinishRendering` are from memory.

**PKResponderState and PKToolPicker**
- `class PKResponderState`, iOS and iPadOS 26.0. Doc text: "Access a responder's PKResponderState through the `pencilKitResponderState` property on any UIResponder. Use it to attach a tool picker to a specific view and control when that picker is visible… Tool picker visibility follows the responder chain… If no responder in the chain specifies a visibility, the tool picker defaults to visible."
- `var activeToolPicker: PKToolPicker?`, get/set, iOS 26.0. "If `toolPickerVisibility` is `.inherited`, the active tool picker is based on the next responder."
- `var toolPickerVisibility: PKToolPickerVisibility`, iOS 26.0, from memory (the symbol page was empty).
- `enum PKToolPickerVisibility`, iOS 26.0, cases:
  - `.inherited`: take the next responder's state.
  - `.hidden`: "active but offscreen, and can appear temporarily in response to user actions".
  - `.inactive`: "No active tool picker."
  - `.active`: the "visible" case (page text not captured).
- `UIResponder.pencilKitResponderState: PKResponderState { get }`, iOS 26.0, nonnull.
- `PKToolPicker.init(toolItems: [PKToolPickerItem])`, iOS 18.0 ("Specify at least one item"; duplicate identifiers show once).
- Items (iOS 18.0):
  - `PKToolPickerItem` (base class)
  - `PKToolPickerInkingItem`
  - `PKToolPickerEraserItem`, with `init(type: PKEraserTool.EraserType, width: CGFloat)`
- `var selectedToolItem: PKToolPickerItem`, iOS 18.0.
- Pre-26 route: `func setVisible(_ visible: Bool, forFirstResponder responder: UIResponder)`, iOS 13.0. It needs a real first responder, which is why the design requires iPadOS 26.
- `func addObserver(_ observer: any PKToolPickerObserver)`, iOS 13.0. Add the canvas and your own observer so tool and colour changes reach you.
- `var showsDrawingPolicyControls: Bool`, iOS 14.0. Set it to false so the system's finger/pencil toggle doesn't contradict R2.

**Strokes**
- `PKStroke.path: PKStrokePath` ("The B-spline path that describes this stroke"), iOS 14.0. `PKStroke.id: UUID` is iOS 27.0 only.
- `PKStrokePath.interpolatedPoints(in range: ClosedRange<CGFloat>? = nil, by strideBy: PKStrokePath.InterpolatedSlice.Stride) -> PKStrokePath.InterpolatedSlice`, iOS 14.0. Stride cases: `.distance(CGFloat)`, `.time(TimeInterval)`, `.parametricStep(CGFloat)`.
- `PKStrokePoint` (iOS 14.0) fields:
  - `location: CGPoint`
  - `timeOffset: TimeInterval` — seconds since stroke start
  - `force: CGFloat` — 1.0 is an average touch, system-defined and not 0 to 1, so normalise before packing into `uint8`
  - `azimuth` and `altitude`, in radians
  - `size` and `opacity`
- `PKDrawing.dataRepresentation() -> Data`, iOS 13.0.
- From memory, not in the page text: apply `stroke.transform` to path points.

**UIPencilInteraction**
- `class UIPencilInteraction`, iOS 12.1.
- `var preferredTapAction: UIPencilPreferredAction`, 12.1, class property.
- `var preferredSqueezeAction: UIPencilPreferredAction`, 17.5, class property.
- `init(delegate: any UIPencilInteractionDelegate)`, 17.5.
- Delegate methods (17.5, optional): `pencilInteraction(_:didReceiveTap:)` and `pencilInteraction(_:didReceiveSqueeze:)`. The types are `UIPencilInteraction.Tap` and `.Squeeze`, both 17.5. The older iOS 12.1 `pencilInteractionDidTap` method is the pre-17.5 fallback.
- `UIPencilPreferredAction` is an enum, iOS 12.1.

**WKWebView snapshot**
- `func takeSnapshot(with snapshotConfiguration: WKSnapshotConfiguration?, completionHandler: @escaping @MainActor @Sendable (UIImage?, (any Error)?) -> Void)` (`NSImage?` on macOS). iOS 11, macOS 10.13.
- `WKSnapshotConfiguration.rect: CGRect` — "in the view's coordinate system"; the default is `CGRectNull`, meaning the whole view.
- `snapshotWidth: NSNumber?` — scales to that width in points, keeping aspect ratio.
- `afterScreenUpdates: Bool` — iOS 13, macOS 10.15.
- **Iframes (Apple does not say).** It is a compositor capture of the visible view `[unverified]`. I found no statement that it excludes sandboxed or cross-origin frames. WebKit's own Site Isolation tracker (WebKit PR #78024, bug 328580) has "takeSnapshot… doesn't capture cross-origin iframes" for the *element-snapshot SPI*, which is a different API. Treat it as the P0 spike the design already names, run with a sandboxed `srcdoc`/opaque-origin page on both iOS 26 and macOS.

**Vision**
- `class VNRecognizeTextRequest`: iOS 13, macOS 10.15.
- `recognitionLevel: VNRequestTextRecognitionLevel` (`.fast` or `.accurate`).
- `recognitionLanguages: [String]` — ISO codes, in priority order.
- `customWords: [String]` — these are ignored if `usesLanguageCorrection` is false.
- Results: `[VNRecognizedTextObservation]`, then `topCandidates(1).first?.string`.
- Run it off the main thread through `VNImageRequestHandler(cgImage:)`.

## 6. Simulator build and verify recipe (knowledge)

The sources are `facts/theuth-ios-simulator-build-on-333`, `procedure/prove-a-theuth-ios-change-on-the-paired-ipad-simulator` and the two warnings below.

- **Devices.**
  - iPad: `glass-198-ipad`, UDID `574E7C28-76CE-4535-AA11-9E0B89D5615B`, iPad Pro 13-inch (M4), iOS 26. It is paired with the 333 engine.
  - iPhone: `glass-198`, UDID `2CEA09C0-FE06-464F-BEF3-2638FA9650E7`.
  - Never erase the iPad or uninstall the app, because that loses the pairing. Re-pairing needs a paste (`facts/theuth-ios-pairing-needs-a-paste`).
- **Quick compile check.**
  `PATH=/Users/sky/.cargo/bin:$PATH cargo check -p desktop --target aarch64-ios-sim` (use `aarch64-apple-ios-sim`), with the worktree's own `CARGO_TARGET_DIR`.
- **Full build, under `/tmp/theuth-build.lock`.**
  `cd crates/desktop && rm -rf gen/apple/build && PATH=/Users/sky/.cargo/bin:$PATH TAURI_FRONTEND_PATH=ui cargo tauri ios build --debug --target aarch64-sim --config tauri.ios.conf.json`
  - Run `rustup target add aarch64-apple-ios-sim` once.
  - `PATH` must put `~/.cargo/bin` first, or the mbx shim kills the build (`warning/mbx-breaks-cargo-tauri-ios-build`).
  - `rm -rf gen/apple/build` is needed because a second build into an existing build dir fails with "Directory not empty (os error 66)" (`warning/tauri-ios-rebuild-over-existing-build-dir-fails`).
  - A fresh worktree needs `bun install` in `crates/desktop/ui`, `packages/apps` and `packages/documents`. It also needs the generated `ui/src/rpc/bindings` copied from a sibling worktree at the same main. The first cold build takes about 35 minutes.
  - Result: `gen/apple/build/arm64-sim/theuth.app`.
- **Install and screenshot.**
  ```
  xcrun simctl boot 574E7C28-76CE-4535-AA11-9E0B89D5615B; xcrun simctl bootstatus 574E7C28-76CE-4535-AA11-9E0B89D5615B
  xcrun simctl install 574E7C28-76CE-4535-AA11-9E0B89D5615B crates/desktop/gen/apple/build/arm64-sim/theuth.app
  xcrun simctl terminate 574E7C28-76CE-4535-AA11-9E0B89D5615B io.theuth.ios; xcrun simctl launch 574E7C28-76CE-4535-AA11-9E0B89D5615B io.theuth.ios
  xcrun simctl io 574E7C28-76CE-4535-AA11-9E0B89D5615B screenshot /tmp/x.png
  ```
  The screenshot is 2064×2752 px, and `idb ui tap` takes points (px / 2). Installing over the old build keeps the pairing.
- **Proof the plugin linked.** `nm -gU <app>/theuth | grep init_plugin_ink`, and `strings` for the Swift class name.
- **`xcrun` shim.** If a Swift Checks harness is compiled on 333, plain `xcrun --show-sdk-path` returns the CommandLineTools SDK and breaks WebKit interfaces. Use the shim from `warning/xcrun-show-sdk-path-returns-commandlinetools-sdk-on-333`.
- **Limitation.** The simulator has no Apple Pencil, so none of the §6.2 exit-gate checks can be proven there (`#L283`). It can prove link, init, overlay mount, z-order and `takeSnapshot`. Pencil behaviour needs a real iPad.
- **Build rewrites the tracked `gen/apple/desktop_iOS/Info.plist`.** Leave that change uncommitted.

## 7. Proposed layout: `crates/tauri-plugin-ink`

```
crates/tauri-plugin-ink/
  Cargo.toml        # name, links = "tauri-plugin-ink"; tauri (workspace, default-features=false);
                    # [target.'cfg(target_os="ios")'.dependencies] tauri features=["wry"];
                    # serde, serde_json, thiserror, log; build-dep tauri-plugin {features=["build"]}
  build.rs          # COMMANDS = [attach, detach, set_enabled, set_tool, set_picker_visible, ack,
                    #   recognize_text, register_listener, remove_listener]
                    # tauri_plugin::Builder::new(COMMANDS).global_api_script_path("./api-iife.js")
                    #   .ios_path("ios").try_build()
  api-iife.js       # a build-script input; generated by rollup in the official plugins
  permissions/default.toml          # default = ["allow-attach", …]
  src/lib.rs        # init() → Builder::new("ink"), invoke_handler, setup → mobile::init / desktop::init
  src/mobile.rs     # ios_plugin_binding!(init_plugin_ink); register_ios_plugin; struct Ink<R>(PluginHandle<R>)
  src/desktop.rs    # stub that returns Unsupported, so `ink:default` on Mac does not fail to load
  src/commands.rs   # #[command] wrappers → run_mobile_plugin("camelName", payload)
  src/models.rs     # serde types: StrokeBatch{ids, strokes[{tool,colour,width,pts:[x,y,force,t_ms]}]},
                    #   ViewportMap{a,b,c,d,tx,ty}, ToolState, PencilEvent{kind: tap|squeeze, action}
  ios/Package.swift # name tauri-plugin-ink; platforms .iOS(.v17); static lib; deps Tauri path
                    # ../.tauri/tauri-api; linkerSettings PencilKit, Vision
  ios/Sources/InkPlugin.swift       # class InkPlugin: Plugin; load(webview:); the @objc commands;
                    # @_cdecl("init_plugin_ink")
  ios/Sources/InkCanvas.swift       # PKCanvasView host; gesture re-parenting; hand-over/ack queue; route-B fallback
  ios/Sources/ToolPickerController.swift  # PKToolPicker(toolItems:); responder-state wiring; picker observer
  ios/Sources/PencilInteraction.swift     # UIPencilInteraction; preferred-action mapping
  ios/Sources/HandwritingRecognizer.swift # VNRecognizeTextRequest
  guest-js/index.ts # attach(), onStrokes(cb), ack(ids), setTool(), onPencil(cb), recognize(png)
  package.json, tsconfig.json       # @tauri-apps/plugin-ink, peer @tauri-apps/api ~2.12
```

Wire-up:
- Root `Cargo.toml` `[workspace.dependencies]`: `tauri-plugin-ink = { path = "crates/tauri-plugin-ink" }`.
- `crates/desktop/Cargo.toml#L63-L66`: add it to the iOS and Android deps table.
- `lib.rs#L109-L111`: add `.plugin(tauri_plugin_ink::init())`.
- `capabilities/mobile.json#L7-L17`: add `"ink:default"`.
- UI: add `@tauri-apps/plugin-ink` and import it dynamically like `lib/haptics.ts`.

Boundary details:
- **Stroke events.** Use a `Channel` passed as an argument to `attach`. Swift decodes it as `Channel` and calls `send(Encodable)`. It is simpler than `trigger` and `registerListener` for high-rate batches. Keep `register_listener` and `remove_listener` in `COMMANDS` only if you use `addPluginListener`.
- **Pencil events.** Tap and squeeze can use `trigger`, with `onPencil` as the listener.
- **Capture.** `wb_capture` stays in `crates/desktop`, using the extended `apple_tile.rs` code. Swift does not take snapshots.
- **Handwriting.** `recognize_text` takes a PNG path or base64 and returns `{text, confidence}`. It can also be done in Rust on iOS with objc2, but Swift is simpler.

## 8. Risks and what a builder must not get wrong

1. **Overlay routing is unproven.** The `hitTest`-nil plus `drawingGestureRecognizer`-on-WKWebView arrangement is only "reported to work" (§6.2, Stack Overflow 66251895). Build the spike first, with route B (own pencil-only recogniser plus `CAShapeLayer`) ready. A moved recogniser reports `location(in:)` in the new view's space, so keep the canvas and web view origins and bounds identical.
2. **Pen events still reach the web view.** The §6.1 capture-phase filter must swallow them, except in eraser mode. With the eraser selected, set `isDrawingEnabled = false` so pen pointer events reach JS, which erases whole strokes. The filter must then pass them.
3. **WKWebView's scroll pan can take Pencil touches.** The UI should disable body scroll, pinch-zoom and rubber-banding. Confirm on iPad that the Pencil never scrolls `WKScrollView`. `[unverified]`
4. **Snapshots and sibling webviews.** `takeSnapshot` of the main `WKWebView` will not contain the raw `WKWebView` tile views from `ios_webview.rs`, because they are siblings. §3.4's "one capture has cards and ink" fails for `web` cards on iOS. Decide whether they count as live cards for ink-on-snapshot, or capture them separately.
5. **Snapshot behaviour to measure.** Run the P0 spikes for sandboxed frames, rect in points vs CSS px, `afterScreenUpdates`, keyboard inset effects, and PNG cost. The PKCanvasView overlay is a sibling and will not appear in the snapshot, which is right because committed ink is web-rendered.
6. **Threading.** Plugin commands arrive on `ipcDispatchQueue`. Hop to main before touching UIKit, and have Vision work off the main thread. `takeSnapshot`'s handler runs on main. `with_webview` closures run on main.
7. **Availability.** Gate `pencilKitResponderState` and `PKToolPickerVisibility` at iOS 26. Gate `UIPencilInteraction(delegate:)`, squeeze and `didReceiveTap` at 17.5, because the app minimum is 17.4. PKStroke.id is iOS 27, so do not use it. Identify strokes by order, as the design says.
8. **Picker.** Create it with `PKToolPicker(toolItems:)` (iOS 18). Set `showsDrawingPolicyControls = false` and add observers. Hide the picker while the composer sheet is open. Read the selected tool and colour from the picker and forward them to JS.
9. **Hand-over correctness.**
   - Use `interpolatedPoints`, not raw control points.
   - Apply `stroke.transform`.
   - Convert with the viewport captured at stroke begin.
   - Normalise `force` (1.0 is average).
   - Keep strokes until acked, with a 300 ms retry.
   - The canvas is append-only.
   - Test 100 fast strokes for no loss and no duplicates.
10. **Build wiring.**
    - The crate needs `links`.
    - Add `register_listener`/`remove_listener` to `COMMANDS`.
    - Link PencilKit and Vision in Package.swift.
    - The tracked `desktop_iOS/Info.plist` is generated; do not commit incidental changes.
    - Put `~/.cargo/bin` first on `PATH`, and `rm -rf gen/apple/build` before rebuilding.
    - Take `/tmp/theuth-build.lock` for full builds.
11. **No on-disk glass reference.** Do not assume `tauri-plugin-glass` code is available. Use haptics 2.4.2 as the template.
12. **Not verifiable on the simulator:** all §6.2 exit-gate checks, hover, squeeze and Pencil latency. They need a real iPad and Pencil.
