# 130 — Companion — as built, W1S (Tauri shell hand-over + bundle nesting, `feat/130-w1s-shell`)

Base `eba311ed`. Rust/Tauri/packaging only (D10, D11, §5.8); macOS-gated, additive. The Swift companion itself is W1
(`crates/desktop/companion/`, not touched here); everything below was exercised with a stand-in bundle (the spike SA
app with `CFBundleIdentifier = io.theuth.mac.companion`, later with a long-lived executable). `DESK/` =
`crates/desktop/src/`, `UI/` = `crates/desktop/ui/src/`.

## Files

| Path | What |
|---|---|
| `DESK/companion/mod.rs` (new) | `BUNDLE_ID = "io.theuth.mac.companion"`, `ALIVE_EVENT = "companion-alive"`, payload `Alive { alive }`, `#[tauri::command] companion_alive() -> bool`; `macos`/`other` backend switch |
| `DESK/companion/macos.rs` (new) | `init`: initial `runningApplications(withBundleIdentifier:)` check → `set_alive`; locate `<exe>/../../Library/LoginItems/Theuth Companion.app`, else `$THEUTH_COMPANION_APP`, else one info log; `NSWorkspace.openApplication(at:configuration:)` with `activates = false`, `addsToRecentItems = false`; `watch` poll (below); `terminate()`: `NSRunningApplication.terminate()` on every instance, `kill(pid, 0)` poll up to 2 s, then `forceTerminate()`; `set_alive`: idempotent (`AtomicBool::swap`), emits `companion-alive`, `tray_by_id("main").set_visible(!alive)` |
| `DESK/companion/other.rs` (new) | Windows/Linux: `init` no-op, `alive() = false`, `terminate()` no-op |
| `DESK/lib.rs` | `mod companion` (desktop); `RunEvent::Exit`: `companion::terminate()` **before** `Launch::shutdown()` (`:52-59`); `tauri_plugin_deep_link::init()` beside `tauri_nspanel::init()` (macOS only, `:281-284`); `companion::companion_alive` appended to `generate_handler!`; `setup`: `companion::init` after `point::init`, then `deep_links(app)` (macOS); `deep_links` → `on_open_url` → `deep_link_route` → `navigate`; `deep_link_route` pure + `#[test] deep_link_routes` (47 tests now, 46 before) |
| `DESK/../Cargo.toml` | desktop table: `tauri-plugin-deep-link = "2"` (same crate the mobile table pins; lock unchanged); macOS table: `objc2-app-kit` features `NSWorkspace`, `NSRunningApplication`, `block2`, `libc`; `objc2-foundation` features `NSURL`, `NSError`; `libc` |
| `crates/desktop/tauri.macos.conf.json` | `plugins.deep-link.desktop.schemes = ["theuth"]` |
| `crates/desktop/capabilities/default.json` | `deep-link:default` appended |
| `UI/lib/companion.ts` (new) | `onCompanionAlive(cb): () => void` (listens `companion-alive`; no-op off Tauri; the returned unlisten works before `listen` resolves), `companionAlive(): Promise<boolean>` → `invoke('companion_alive')` |
| `scripts/package-macos.sh` | step 1: `scripts/bundle-companion.sh` when `crates/desktop/companion/Package.swift` exists, else a skip line; step 3: `ditto target/release/companion/Theuth Companion.app` → `$APP/Contents/Library/LoginItems/`, else a skip line; step 4: companion `codesign --force --options runtime $TIMESTAMP_FLAG -i io.theuth.mac.companion --entitlements crates/desktop/companion.entitlements.plist` **before** the outer `.app`; `codesign --verify --deep --strict` kept; "companion:" line in the done summary |
| `crates/desktop/companion.entitlements.plist` (new) | empty `<dict/>` — hardened runtime with no entitlements (spike SA §3 step 5: status item, panel, notifications need none; Swift needs no JIT) |

Not touched: `UI/App.tsx`, `UI/store/*`, `UI/lib/tauri.ts`, `Info.plist`, `tauri.conf.json`, `tauri.bundle.conf.json`,
`capabilities/mobile.json`, `tauri.ios.conf.json`, anything under `crates/desktop/companion/`.

## Deep-link facts verified

- **The bundler writes `CFBundleURLTypes`; `Info.plist` is left alone.** tauri-cli 2.11.4 reads
  `plugins.deep-link.desktop` into `settings.deep_link_protocols` (`tauri-cli/src/interface/rust.rs:887-900`) and
  tauri-bundler 2.9.4 inserts `CFBundleURLTypes` with `CFBundleURLSchemes`, `CFBundleURLName` (default
  `<bundle id> <scheme>`), `CFBundleTypeRole` (`bundler/src/bundle/macos/app.rs:297-331`); the user `Info.plist`
  is merged *after* that (`:352-360`), so a hand-written entry would override, not add. One source: the config.
- **Scheme config lives in `tauri.macos.conf.json`, not `tauri.conf.json`** (deviation from the brief's wording).
  The base file is merged under every platform's file; a `desktop` entry there would make the Windows installer and
  the Linux `.desktop` register `theuth://` too, and without `tauri-plugin-single-instance` each link would start a
  second process (the plugin's desktop path reads the link from `argv`, `tauri-plugin-deep-link/src/lib.rs:196-228`).
  §5.9 says Windows/Linux keep today's behaviour, so the scheme is macOS-only; `mobile`/iOS config is untouched
  (`tauri.ios.conf.json` keeps `plugins.deep-link.mobile` for `relay.theuth.io/link`; `mobile::configure` still
  registers the plugin; `capabilities/mobile.json` unchanged).
- **Runtime path on macOS:** `RunEvent::Opened { urls }` → the plugin emits `deep-link://new-url` → `on_open_url`
  (`lib.rs:520-545`). `deep_link_route`: `theuth://attention/<id>` → `/attention/<id>`, `theuth://c/<session>` →
  `/c/<session>`, `theuth://new` → `/new` (the brief; §5.8 said `/`; `routes/new.tsx` exists). Anything else —
  other host, empty id, extra path on `new`, other scheme — logs `deep link ignored: <url>` and drops. `navigate`
  shows + focuses the main window as before.
- **Not exercised end to end:** a `theuth://` click needs a bundled `.app` with the generated `Info.plist`
  (`cargo tauri build` is out of this wave's gates); the unbundled debug binary has no `CFBundleURLTypes`. The route
  parser is unit-tested; the plugin wiring is the stock one.

## Deviation: alive detection polls instead of observing

§5.8 and the brief name `NSWorkspace.didLaunchApplicationNotification` / `didTerminateApplicationNotification`.
AppKit does not post either for `LSUIElement` apps (Apple's `didLaunchApplicationNotification` page: not posted for
background apps or apps with `LSUIElement`; Apple points at KVO on `NSWorkspace.runningApplications` instead). Verified
on this Mac (26.6.2): a bare Swift observer saw `com.apple.calculator` launch/quit and saw nothing for the spike
bundle, with or without `LSUIElement`. Implemented instead: a `tauri::async_runtime` task ticks every 2 s and runs
`set_alive(!runningApplications(withBundleIdentifier:).isEmpty)` on the main thread; our own launch flips alive from
the `openApplication` completion handler, so the common path has no 2 s overlap of two menu bar icons. Marked
`ponytail:` in `macos.rs`; the push upgrade is a KVO observer class (`define_class!`) on `runningApplications`.

## Other deviations

- Command is `companion_alive() -> bool` (brief) rather than §5.8's `companion_status() -> { alive, version }`.
- `companion::terminate()` runs in `RunEvent::Exit` before `Launch::shutdown()` — i.e. after the webview's RPC
  `shutdown` + `quit_ack` (or the 3 s fallback), before the sidecar pty hang-up — the order the brief asks for.
  §5.8's "before `quit-requested`" would have killed the rail while the engine was still up.
- No `bundle.macOS.files`: the bundler copies custom files recursively but never adds them to `sign_paths`
  (spike SA §3), and `package-macos.sh` already signs by hand after `cargo tauri build`, so the `ditto` + explicit
  `codesign` keep one signing order. The dev-run cost (no helper in a bare `cargo tauri dev`) is covered by
  `$THEUTH_COMPANION_APP`.
- The companion is signed once as a bundle (one `codesign` line, not executable + bundle): `codesign` on a bundle
  signs its `CFBundleExecutable` and seals `Info.plist`/`Resources`; the helper has no nested code.

## Smoke runs

Debug binary (`cargo build -p desktop --bin theuth-desktop`, no dev server, `RUST_LOG=desktop_lib=debug`) with
`THEUTH_COMPANION_APP=/tmp/130-w1s-dryrun/target/release/companion/Theuth Companion.app` (stand-in: the spike's
bundle with the id changed and a 10-line long-lived `NSApplication` executable, ad-hoc signed — an unsigned copy
fails `openApplication` with "a miscellaneous error occurred", so the ad-hoc dev signature in
`bundle-companion.sh` matters):

```
11:51:41.955 companion: launched …/Theuth Companion.app
11:51:41.955 companion: alive=true; tray hidden
   (pkill CompanionSpike)
11:51:47.815 companion: alive=false; tray shown          ← poll, ≤ 2 s
   (open -g …/Theuth Companion.app)
11:51:51.815 companion: alive=true; tray hidden          ← poll
   (NSRunningApplication(pid).terminate() on theuth-desktop → RunEvent::Exit)
terminate() sent to 36808: true
desktop exited rc=0 after .356 s                        ← companion gone, no forceTerminate warning
--- companion after quit: none
```

Packaging dry run — the two new blocks of `package-macos.sh` run verbatim against a scratch outer app
(`/tmp/130-w1s-dryrun/theuth.app`, the spike's `Outer.app`; never the installed one), `IDENTITY=-`:

```
==> [3/5] Theuth Companion.app → Contents/Library/LoginItems
==> [4/5] codesign (identity: -, hardened runtime)
/tmp/130-w1s-dryrun/theuth.app: replacing existing signature
--prepared:…/theuth.app/Contents/Library/LoginItems/Theuth Companion.app
--validated:…/theuth.app/Contents/Library/LoginItems/Theuth Companion.app
/tmp/130-w1s-dryrun/theuth.app: valid on disk
/tmp/130-w1s-dryrun/theuth.app: satisfies its Designated Requirement
VERIFY-OK                                        ← codesign --verify --deep --strict --verbose=2
Identifier=io.theuth.mac.companion
CodeDirectory v=20500 … flags=0x10002(adhoc,runtime)
```

`CodeResources` of the outer app carries `Library/LoginItems/Theuth Companion.app` with `cdhash` + `requirement`;
the companion's entitlements blob is the empty dict. `bash -n scripts/package-macos.sh` clean.

Gates: `cargo fmt -p desktop` · `cargo clippy -p desktop -- -D warnings` clean · `cargo check -p desktop` ·
`cargo test -p desktop` 47 passed · `companion.ts` type-checks under the project `tsconfig.json`
(`tsc -p` with an include of the one file; `bun install` was needed first in this fresh worktree).

## Integration step (sibling-owned, not done here)

`UI/App.tsx`: `import { companionAlive, onCompanionAlive } from './lib/companion'`; hold `alive` in a ref or store
(`companionAlive()` once at boot, `onCompanionAlive` for changes); skip `notifyIfHidden(...)` in the
`ApprovalRequested` / `TurnFinished` handlers (`App.tsx:366-378`) and pass `null` to
`TrayIcon.getById('main').setTitle(...)` (`:386-391`) while `alive` (D8: the companion owns banners and the menu
bar count while it runs). Windows/Linux never receive `alive = true`, so nothing changes there.

## Open

- `theuth://` end-to-end and `CFBundleURLTypes` in the generated `Info.plist`: check on the first `just package-macos`
  with this branch (`/usr/libexec/PlistBuddy -c 'Print :CFBundleURLTypes' theuth.app/Contents/Info.plist`, then
  `open 'theuth://new'`).
- `scripts/bundle-companion.sh` (W1) must ad-hoc sign its product for dev runs; the release signature is replaced in
  step 4 regardless.
