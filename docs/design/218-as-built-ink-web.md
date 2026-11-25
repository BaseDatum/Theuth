# 218 — as built: web-only ink (R13)

Branch `feat/218-web-ink`, off `main` at `b4a960243` (tagged `ink-hybrid-final`: the native hybrid is one checkout away). Decision: `218-recon/ink-decision.md`. Design: `218-whiteboard.md` R13, §6.1, §6.2 (retired), §6.6; contract §6 (superseded note).

## Deleted

- **Native capture**: `CaptureSurfaces.swift`, `InkOverlay.swift`, `ToolPickerController.swift` (the `PKToolPicker`), PencilKit linkage, and every Swift/Rust model of the hand-over (`NativeStroke`, `StrokeBatch`, `AttachResult`, `Tool`, `State`/`Tool` events, …).
- **Plugin commands** `attach`, `detach`, `set_enabled`, `set_viewport`, `set_tool`, `set_picker_visible`, `ack`: their Rust/Swift handlers, `build.rs` list, permissions, `api-iife.js` / `guest-js` wrappers and tests (`stroke_batch_serde_shape`, `attach_reports_desktop`).
- **UI**: `components/whiteboard/native-ink.ts` (+ test), `pen-latch.test.ts`, the fake-plugin parts of `pen-latch-canvas.test.ts`, `inkEngine` / `nativeInk` (store, `RouteState`), `commitCaptured` (Canvas, `WhiteboardCanvasHandle`), the "Native Pencil ink" row in ⋯, the native attach/visibility/viewport wiring in `WhiteboardView.tsx`.

## What remains of `tauri-plugin-ink`

Three commands: `pencil_listen { onEvent }` → `{ ok }` (installs one stateless `UIPencilInteraction` forwarder, `PencilController.swift`, `@available(iOS 17.5, *)`; `ok: false` off iPad / below 17.5 / desktop), `pencil_unlisten`, `recognize_text` (Vision, unchanged). Events: `{ kind: 'pencil', gesture, action }` and `{ kind: 'error' }`. Native code holds no tool state.

## Web path

- `lib/ink-native.ts`: `recognizeText(png)` and `listenPencil(on)` only; failure-tolerant; only the latest listener may unlisten.
- `pencil-gestures.ts`: `switchEraser` toggles erase ↔ draw, `switchPrevious` swaps with the previous tool (tracked from store changes), the three palette actions raise the cluster, `runSystemShortcut` / `ignore` do nothing.
- `WhiteboardView.tsx`: `recognize: recognizeText` passed to `sendWhiteboard` unconditionally (it was only passed while native ink was attached, so the toggle off also turned handwriting off).
- `canvas/Canvas.tsx`: a pen-down (pointer or page relay) clears a resting palm's long-press and cancels `MultiTap`; no `taps.down` while `palmActive`, and a multi-tap ending in the palm window does not undo/redo. The wet stroke keeps the predicted tail on `Wet.predicted` and draws once per animation frame (`scheduleWet`); predicted points are never committed. `StrokeSamples`, coalesced events and pressure unchanged. Tilt is not captured (no tool uses it; `Pt` unchanged).
- The cluster's picker is the only tool UI; the tool latches until another tool, Select or Esc.

## Tests

Added: `canvas/palm-and-wet.test.ts` (palm-first then pen: no mark menu, no undo, view still; two-finger tap in / after the palm window; 600-sample doubled-move stroke with predicted samples: ≤ 1 wet draw a frame, no predicted point committed, one stroke, one path — all three fail on the pre-change Canvas), `pencil-gestures.test.ts`, `lib/ink-native.test.ts` (rewritten), `send/send.test.ts` "handwriting goes with the plugin present and nothing attached", `pen-latch-canvas.test.ts` (Pencil and mouse, several strokes each; two cases ported from `pen-latch.test.ts`), `pencil_listen_reports_desktop` (Rust).
Removed: `native-ink.test.ts`, `pen-latch.test.ts`, the native-ink case of `canvas/input.test.ts`, `stroke_batch_serde_shape`, `attach_reports_desktop`.

## Checks

All exit 0 unless noted:

1. `bun run check` (tsc) and `bun x vitest run src/components/whiteboard src/lib`: 163 files, 1 167 tests passed.
2. `cargo clippy -p tauri-plugin-ink --all-targets -- -D warnings`; `cargo test -p tauri-plugin-ink -- --test-threads=4`.
3. `cargo check -p desktop` (host) and `cargo check -p desktop --target aarch64-apple-ios-sim`. The iOS check passes only with `IPHONEOS_DEPLOYMENT_TARGET=17.4`; without it swift-rs builds the package for iOS 13 and the unchanged `HandwritingRecognizer.swift:34` (`automaticallyDetectsLanguage`, iOS 16) fails. This was already true before the change: the check had never been run (as-built B5). `cargo tauri ios build` sets the target and builds.
4. `swiftc -typecheck` of `ios/Sources/*.swift` against iPhoneSimulator 26.0 (`xcrun --sdk iphonesimulator`), with the Tauri module built from the registry `tauri-2.12.5` sources and a one-class `SwiftRs` stub. A deliberate error was reported, so the check is live.
5. The grep from the brief (check 6) over the plugin, `components/whiteboard` and `lib/ink-native*` finds nothing.
6. iPad simulator (`glass-198-ipad`): `cargo tauri ios build --debug --target aarch64-sim` passed. The binary links `init_plugin_ink`, contains `PencilController`, and has no `PKCanvasView` / `PKToolPicker` strings. Installed over the old build, so the pairing is kept. A new whiteboard opens, and a finger swipe pans it: the canvas pixels change, and a screenshot with no gesture changes nothing. Screenshots are in `/tmp/218webink/`. The simulator has no Pencil.

## Only a real iPad can prove (for Sky)

1. Three strokes after a tool pick all draw with that tool.
2. A Pencil tap on a cluster button is a tap.
3. A resting palm neither pans, opens a menu nor undoes.
4. Pencil double-tap toggles the eraser (if not: delete `PencilController` and the two `pencil_*` commands).
5. A mark sent alone carries handwriting text.
6. The 240 fps latency number (baseline; ≤ 12 frames fine, > 16 a regression — `ink-decision.md` §4).
