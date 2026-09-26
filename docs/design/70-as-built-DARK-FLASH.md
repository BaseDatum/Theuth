# DARK-FLASH as built — white flash on launch in dark mode

Branch `fix/dark-mode-launch-flash`, worktree `theuth-darkflash`, base `893187a`. Two commits plus this report.

Defect (user): "Sometimes when I open the mobile app in dark mode I get a white theme flash as the app opens."

## Root cause

Four layers were checked; two were already closed on `main`, two were open.

| Layer | State on `main` | Fix |
|---|---|---|
| 1. Native — WKWebView is `isOpaque` white until the page's first paint; nothing set `backgroundColor` on the window or webview (no `backgroundColor` in any `tauri*.conf.json`, no `.background_color()` on the iOS main builder or POPOUT's `open_window`). The LaunchScreen storyboard already used `systemBackgroundColor` (dynamic). | **open** — this is the flash | `f02f696` |
| 2. Document — `index.html` had `<meta name="color-scheme" content="light dark">` and a blocking `<script src="/theme.js">` (external: both CSPs are `script-src 'self'`) that sets `.dark` on `<html>` before first paint from `localStorage.theme` ∈ `dark`/`light`/else `prefers-color-scheme`. But `<html>` had no background of its own; `body { @apply bg-background }` lives in `styles.css`, a `<link>` in prod and JS-injected in dev. | half open | `9a3d826` |
| 3. App — `initTheme()` runs synchronously at the top of `main.tsx` (before `createRoot`), no `useEffect`; there is no theme provider, the theme is the `.dark` class on `<html>`. Island and pop-outs load the same bundle and `index.html`. | closed | none |
| 4. Splash — `StartupScreen.tsx` wrappers are `bg-background text-foreground`, cards `bg-elevated`, log `bg-layer-2`; `GuestApp` screens rely on body `bg-background`. No hard-coded white. | closed | none |

## Commits

### `f02f696` fix(desktop): paint page colour under the webview before first paint

- `crates/desktop/src/lib.rs`
  - `page_background(dark) -> tauri::window::Color`: `--bg-page` from `ui/src/styles.css` — soot `#12100e` / bone `#e8dcc8`.
  - iOS (`mod mobile`): the main `WebviewWindowBuilder` gets `.background_color(page_background(system_dark()))`. wry then sets `isOpaque = false`, `backgroundColor` on the webview and on the containing `UIView`, and `drawsBackground = NO` on the configuration (wry 0.55.1 `wkwebview/mod.rs:367-384, 447-471`; the `transparent` feature is on through tauri's `macos-private-api`). `system_dark()` reads `UIScreen.mainScreen.traitCollection.userInterfaceStyle` because tao's iOS `Window::theme()` is a constant `Light` (tao 0.35.3 `platform_impl/ios/window.rs:506`). `setup` runs at `RunEvent::Ready`, i.e. inside the running `UIApplication`, so the screen trait collection is populated.
  - macOS (`mod desktop`): `paint_page_background(&WebviewWindow)` = `set_background_color(Some(page_background(theme == Dark)))` on window + webview; skips `island` (transparent). Called for `main` in `setup` (the window is `visible: false` until the tray shows it, so this lands before any paint) and from `on_window_event` on `WindowEvent::ThemeChanged` for every window, so a system appearance switch while running does not leave a stale colour behind the page or in the overscroll (`underPageBackgroundColor`).
- `crates/desktop/src/commands.rs` `open_window`: the pop-out builder gets `.background_color(page_background(dark))` with `dark` read from `main`'s `theme()` (the pop-out does not exist yet). `ThemeChanged` covers it afterwards.
- `crates/desktop/Cargo.toml`: `objc2-ui-kit` features `UIScreen`, `UITraitCollection`, `UIInterface` (iOS only).
- Not done: a `backgroundColor` in `tauri.conf.json`. It is one static colour; light-mode users would get a soot frame instead. Runtime `set_background_color` in `setup` gives the same effect keyed on the actual appearance.

### `9a3d826` fix(ui): give <html> --bg-page before the stylesheet loads

- `crates/desktop/ui/index.html`: after `theme.js`, an inline `<style>` (`style-src 'unsafe-inline'` is already in both CSPs): `html { background: #e8dcc8 } html:where(.dark) { background: #12100e }`. Keyed on the class `theme.js` just set, so the app's stored choice wins over the system's, not only `prefers-color-scheme`. `:where` keeps its specificity at `0,0,1` so `styles.css`'s `html[data-island] { background: transparent }` wins regardless of stylesheet order (Vite appends the `<link>` after the inline style in prod; dev injects `<style>` later — both fine, `:where` removes the dependency).
- `crates/desktop/ui/src/lib/theme.test.ts`: runs the real `public/theme.js` through `new Function` with stubbed `localStorage`/`matchMedia`/`document` for stored ∈ {`dark`, `light`, `system`, unset} × prefers-dark ∈ {false, true}: `dark` → dark, `light` → light, `system`/unset → follows prefers-dark. 8 cases.
- The brief asked for an `@media (prefers-color-scheme: dark)` inline rule. Not added: with `theme.js` running first, the class is always present pre-paint, and a media rule would paint stored=light on a dark system dark for a frame. The media query is the fallback only if JS failed, in which case `initTheme()` fails too and nothing is dark anyway.

## Measurement

macOS, system appearance Dark, `cargo tauri dev --no-watch` from `crates/desktop` with `THEUTH_DATA_DIR=/tmp/69/darkflash-dd`, `THEUTH_BIN=<nonexistent>`, `TAURI_FRONTEND_PATH=ui`, `RUST_LOG=warn`. `screencapture -V 6` (2560×1440, VFR ≈ 45 fps), pop-out opened from the chat header's "Open in new window", frames decoded with `-fps_mode passthrough`, region = the pop-out's content (900×600 inside 960×720 at 800,203), a frame counts as white when > 50 % of its pixels have `min(r,g,b) > 235`.

| build | frames | white frames |
|---|---|---|
| before (`open_window` without `background_color`, no inline style) | 273 | **3** (frames 86–88: window appears dark from `NSWindow`'s dynamic background in 85, then WKWebView white ×3, then the page) — `/tmp/69/darkflash-popout-before.mov`, `/tmp/69/darkflash-before-frames85-89.png` |
| after | 283 | **0** (no frame even reaches > 50 % pixels with `min > 120`) — `/tmp/69/darkflash-popout.mov` |

Main window: `RUST_LOG=warn` logged nothing (a failed `set_background_color` would log `window background`); shown via the tray's Open, `/tmp/69/darkflash-main.png` samples centre and bottom-right at `(18,16,14)` = `#12100e`. The main window cannot flash on macOS by construction — it is `visible: false` until the tray shows it — so no before/after there.

iOS: not measured. This machine has Command Line Tools only (no Xcode, no simulator runtime; `cargo check --target aarch64-apple-ios` stops at `objc2-exception-helper`'s build script for want of the SDK). Static evidence: the wry iOS path quoted above is unconditional on `background_color.is_some()`, and the same builder call fixed the identical WKWebView-on-macOS case above. The iOS branch of `system_dark()` is checked against objc2-ui-kit 0.3.2 signatures (`UIScreen::mainScreen(MainThreadMarker) -> Retained<UIScreen>`, `UITraitEnvironment::traitCollection`, `unsafe fn userInterfaceStyle -> UIUserInterfaceStyle` with `PartialEq`) but has not been compiled here.

## Gates

`bunx tsc --noEmit` 0 · `bunx vite build` 0 (dist `index.html` carries the inline style after `theme.js`, before the `<link>`s) · `CI=1 bunx vitest run` 86 files / 516 tests green · `cargo check -p desktop` clean (host).

## Known ceilings

- Native colour is the **system** appearance; a user who forces `dark` on a light system gets a bone native frame before `theme.js` paints soot (and vice versa). The native side cannot read the webview's `localStorage`. Closing it would need the preference mirrored to the Rust side (a Tauri store) — not worth it until someone reports it.
- iOS has no `ThemeChanged`; the native colour is fixed at launch. The page itself follows the OS via `initTheme()`'s `matchMedia` listener, so only the overscroll colour can go stale until relaunch.
- Pre-existing rustfmt drift in `crates/desktop/src` (island/*, launch.rs, models.rs, update.rs) is untouched; the two new long lines follow the file's existing width.

DONE DARK-FLASH
