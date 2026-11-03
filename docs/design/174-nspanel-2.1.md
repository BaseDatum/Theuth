# 174 NSPANEL-2.1: crates/desktop on tauri-nspanel 2.1.1 (objc2), as built

Branch `feat/174-nspanel-2.1`, cut from main `1f921ce4`.

## Ask (Sky, 2026-11-03)

- He read the last remaining build note, "the following packages contain code that will be rejected by a future version of Rust: block v0.1.6".
- He asked "what does the tauri-nspanel update provide? is there a risk in updating it?". He was told: off a frozen branch, onto the maintained objc2 rewrite; it clears the note and brings panel fixes; the risk is behavioural (focus, Spaces, click-through), not compile-time.
- Then he said: "go ahead and do it now."

## Facts

- **Old pin.** Root `Cargo.toml` had `tauri-nspanel = { git = "https://github.com/ahkohd/tauri-nspanel", branch = "v2" }`, locked at `18ffb9a2` (2025-06-15, package 2.0.1). `block 0.1.6` came only through it: directly and via `cocoa 0.26.1`, `cocoa-foundation 0.2.1`, `objc-foundation 0.1.1` (doc 168).
- **Old class** (`raw_nspanel.rs` at `18ffb9a`): `RawNSPanel : NSPanel` overrides two selectors, `canBecomeKeyWindow` → `YES` (`:48`) and `dealloc` → `[super(NSObject) dealloc]` (`:52-59`). `from_window` (`:251-267`) also adds an `NSTrackingArea` (options `0x87`, owner = content view, rect = content bounds at swizzle time) and sets the content view and its subviews' autoresizing mask to width | height.
- **New crate** (crates.io `tauri-nspanel 2.1.1`, objc2 0.6 / objc2-app-kit 0.3 / objc2-foundation 0.3, `tauri >= 2.8.5` with `macos-private-api`, which theuth already enables): one `NSPanel` subclass per `tauri_panel!` invocation (ObjC class name = the Rust name), whose `config` keys become `BOOL` selector overrides. `to_panel::<P>()` returns `PanelHandle<R> = Arc<dyn Panel<R>>` and stores it for `ManagerExt::get_webview_panel`. `from_window` swizzles, applies the config, adds the tracking area only when `with: tracking_area` is given, sets subviews' autoresizing mask, and installs `-windowLevel` on the `WKWebView` subclass (IME candidate windows above elevated panels). `set_style_mask` catches AppKit exceptions and returns `Result<(), StyleMaskError>`, and syncs `_setPreventsActivation:` with the nonactivating bit (activation fix). No `dealloc` override (the class keeps `NSPanel`'s). `to_window` restores ownership (#129; unused here).
- **Main thread in 2.1.** Panel methods carry no `MainThreadMarker` and no runtime check; the wrapper is `unsafe impl Send + Sync` and its doc says callers must run panel operations on the main thread. Only `PanelBuilder` (`builder.rs:984`, `:1042`) and `panel_event!` handlers (`event.rs:190`, `expect("Must be on main thread")`) check; theuth uses neither.

## What changed

| File | Change |
|---|---|
| `Cargo.toml` | `tauri-nspanel = "2.1.1"  # 53, 174`; comment no longer mentions the old `cocoa`/`objc` crates |
| `Cargo.lock` | see "Cargo.lock diff" |
| `crates/desktop/src/island/macos.rs` | `PanelSpec` (+ `apply`), `TRACKING`, island `SPEC`, `mod class { tauri_panel!(TheuthIslandPanel …) }`; `init` uses `to_panel::<TheuthIslandPanel>()` + `SPEC.apply`; `conceal` takes `&PanelHandle<Wry>`; `set_ignore_mouse_events` → `set_ignores_mouse_events`; the `#[allow(deprecated)]` and `cocoa` import are gone; 2 tests |
| `crates/desktop/src/point/macos.rs` | overlay `SPEC`, `mod class { tauri_panel!(TheuthOverlayPanel …) }`; `panelize` via `to_panel::<TheuthOverlayPanel>()` + `SPEC.apply`; same renames; 1 test |
| `crates/desktop/src/lib.rs` | unchanged: `builder.plugin(tauri_nspanel::init())` (`:277`) has the same signature in 2.1 |

## API mapping, per call site

| Call site | 2.0 (git v2) | 2.1.1 |
|---|---|---|
| `lib.rs:277` | `tauri_nspanel::init()` | `tauri_nspanel::init()` (same) |
| `island/macos.rs` `init` | `win.to_panel()` → `ShareId<RawNSPanel>` | `win.to_panel::<class::TheuthIslandPanel>()` → `PanelHandle<Wry>` |
| `island` `init`, `point` `panelize` | `panel.set_style_mask(i32)` | `panel.set_style_mask(NSWindowStyleMask)?` (in `PanelSpec::apply`) |
| same | `panel.set_level(i32)` | `panel.set_level(i64)` |
| same | `panel.set_collection_behaviour(cocoa NSWindowCollectionBehavior)` | `panel.set_collection_behavior(objc2_app_kit::NSWindowCollectionBehavior)` |
| same | `set_hides_on_deactivate(false)`, `set_becomes_key_only_if_needed(true)` | same names, same values |
| `island` `conceal`, `island_show`; `point` `panelize`, `set_revealed` | `set_alpha_value(f64)` | same |
| same | `set_ignore_mouse_events(bool)` | `set_ignores_mouse_events(bool)` |
| `island_show`, `point` `set_revealed` | `order_front_regardless()` | same |
| `island_resign_key` | `resign_key_window()` | same |
| `island_show`, `island_resign_key`, `island_hide`, `point` `set_revealed` | `get_webview_panel(label)` → `ShareId<RawNSPanel>` | `get_webview_panel(label)` → `PanelHandle<Wry>` (same `Error::PanelNotFound`) |
| `conceal(panel: &tauri_nspanel::Panel)` | `Panel = ShareId<RawNSPanel>` | `conceal(panel: &PanelHandle<tauri::Wry>)` |
| both files | `use tauri_nspanel::cocoa::appkit::NSWindowCollectionBehavior` (+ `#[allow(deprecated)]`) | `use tauri_nspanel::panel::{NSWindowCollectionBehavior, NSWindowStyleMask, …}` (the crate's objc2 re-exports) |
| panel class | shared `RawNSPanel` | `TheuthIslandPanel` / `TheuthOverlayPanel` via `tauri_panel!` |

## Parity, per property per panel

The tests `island::macos::tests::island_spec_is_the_pre_port_panel` and `point::macos::tests::overlay_spec_is_the_pre_port_panel` compare the whole `PanelSpec` to a literal built from raw numbers; `tracking_area_is_the_pre_port_one` pins `0x87`.

| Property | Island old | Island new | Overlay old | Overlay new |
|---|---|---|---|---|
| `canBecomeKeyWindow` | YES (`RawNSPanel` override) | YES (`config: can_become_key_window`) | YES | YES |
| `dealloc` | overridden: `[super(NSObject) dealloc]` | `NSPanel`'s (no override) | same as island | same as island |
| style mask | `1<<7 \| 1<<3` = 136 (i32) | `NonactivatingPanel \| Resizable` = 136 | `1<<7` = 128 | `NonactivatingPanel` = 128 |
| level | 25 (i32) | 25 (i64) | 101 | 101 |
| collection behaviour | CanJoinAllSpaces \| Stationary \| FullScreenAuxiliary = 1+16+256 = 273 | 273 | + IgnoresCycle = 337 | 337 |
| hidesOnDeactivate | false | false | false | false |
| becomesKeyOnlyIfNeeded | true | true | true | true |
| tracking area | 0x87 on content view, swizzle-time rect | 0x87, `auto_resize: false` (no `InVisibleRect`) | same | same |
| subviews' autoresizing | width \| height | width \| height | same | same |
| content view autoresizing | width \| height | not set | same | same |
| order of setup | to_panel, style, level, Spaces, deactivate, key, (capture hide), show, alpha 0, ignore mouse | same (`PanelSpec::apply` is the middle five) | to_panel, style, level, Spaces, deactivate, key, show, alpha 0, ignore mouse | same |
| concealed | alpha 0, ignores mouse | same | alpha 0, ignores mouse | same |
| revealed | alpha 1, mouse on, orderFrontRegardless | same | ignore = !(on && catch_mouse), alpha 1/0, orderFrontRegardless if on | same |
| `_setPreventsActivation:` | never sent | sent with YES (nonactivating bit set) | never sent | sent with YES |
| `-windowLevel` on the webview class | absent | installed (returns the window's level) | absent | installed |
| other `NSPanel` selectors | default | default; the class also implements `mouseEntered:`/`mouseExited:`/`mouseMoved:`/`cursorUpdate:` forwarding to the delegate only if it responds | same | same |

## Main thread

No call needed a change: every panel call already runs on the main thread, and 2.1 rejects none of them.

| Call site | Panel calls | Runs on | How we know | 2.1 requirement | Change |
|---|---|---|---|---|---|
| `lib.rs:277` `tauri_nspanel::init()` | plugin setup: `app.manage(WebviewPanelManager)` | main (builder, before `run`) | Tauri builder | none (no AppKit) | none |
| `island/macos.rs` `init` | `to_panel`, `apply` (5 setters), `set_alpha_value`, `set_ignores_mouse_events` | main | `.setup` hook, `lib.rs:340` | main thread (doc), unchecked | none |
| `island_show` | `get_webview_panel`, `set_alpha_value`, `set_ignores_mouse_events`, `order_front_regardless` | main | sync `#[tauri::command]`: Tauri runs non-`async` commands on the main thread; `island_place` already asserts it with `MainThreadMarker::new()` | same | none |
| `island_resign_key` | `get_webview_panel`, `resign_key_window` | main | sync command | same | none |
| `island_hide` | `get_webview_panel`, `set_alpha_value`, `set_ignores_mouse_events` | main | sync command | same | none |
| `point/macos.rs` `panelize` | `to_panel`, `apply`, `set_alpha_value`, `set_ignores_mouse_events` | main | only caller `overlay_ensure_on_main` (`point/mod.rs:615`), reached only through `on_main` (`run_on_main_thread`, `:558-569`) from the `overlay_ensure` command (`:183`), the screens-changed refit (`:101`) and voice-key reveal (`:286`) | same | none |
| `point/macos.rs` `set_revealed` | `get_webview_panel`, `set_ignores_mouse_events`, `set_alpha_value`, `order_front_regardless` | main | `overlays_set_revealed` via `on_main` (`:288`, `:407`); vanished-display conceal inside `overlay_ensure_on_main` (`:625`) | same | none |
| `get_webview_panel` anywhere | `Mutex` lookup in managed state | any | `lib.rs:274-283` of the crate | none | none |
| `point/macos.rs` `left_button_down` (not panel API) | `NSEvent::pressedMouseButtons` | tokio worker (`sample`, `point/mod.rs:511`) | unchanged by this port | n/a | none |

## Cargo.lock diff

`git diff --stat Cargo.lock`: 10 insertions, 99 deletions.

- Removed packages: `block 0.1.6`, `cocoa 0.26.1`, `cocoa-foundation 0.2.1`, `core-graphics 0.24.0`, `malloc_buf 0.0.6`, `objc 0.2.7`, `objc-foundation 0.1.1`, `objc_id 0.1.1`, `tauri-nspanel 2.0.1 (git v2#18ffb9a2)`.
- Added: `tauri-nspanel 2.1.1` (registry), depending on `objc2`, `objc2-app-kit`, `objc2-foundation`, `pastey 0.2.3`, `tauri`, all already in the lock.
- Edges: `agent-desktop-macos`, `enigo` and `tao` list `"core-graphics"` instead of `"core-graphics 0.25.0"`: the disambiguating suffix goes because 0.24.0 is gone. They still resolve to 0.25.0.
- No other package changes version and no other edge moves. Checked with `cargo metadata --locked` (rc 0, lock byte-identical afterwards).

## Gates

| # | Command | Result |
|---|---|---|
| 1 | `cargo fmt -p desktop` | clean; `git status` afterwards lists only the four touched files |
| 2 | `BUILD_LABEL=174 ~/.theuth/bin/build-lock mbx check -p desktop --message-format short > /tmp/174/check.log 2>&1` | rc 0; 0 `warning` lines from `crates/`; `grep -c 'future version of Rust' /tmp/174/check.log` = 0 |
| 3 | `BUILD_LABEL=174 ~/.theuth/bin/build-lock mbx clippy -p desktop --all-targets --no-deps -- -D warnings` | rc 0, no warnings (`/tmp/174/clippy.log`) |
| 4 | `BUILD_LABEL=174 ~/.theuth/bin/build-lock mbx test -p desktop --lib --no-run`, then `mbx test -p desktop --lib` | rc 0 both; 64 passed, 0 failed, including the 3 new spec tests (`/tmp/174/test.log`) |
| 5 | `cargo tree --offline -i block\|cocoa\|objc-foundation --target aarch64-apple-darwin` | each: "package ID specification … did not match any packages" (`/tmp/174/tree.log`) |
| 6 | `cargo tree --offline -i tauri-nspanel --target x86_64-pc-windows-msvc` | "nothing to print" (absent); on aarch64-apple-darwin only `desktop` depends on it |

## Deviations and decisions

- **Hand-pruned lock instead of `cargo update -p tauri-nspanel --precise 2.1.1`.** That command (and a plain re-resolve after the `Cargo.toml` edit) also moved 22+ unrelated edges: `windows-sys 0.52/0.59/0.60 → 0.61.2` in errno, rustix, socket2, tempfile and others; `syn 1/2 → 3.0.9` in `data-encoding-macro-internal` and `cssparser-macros`; `itertools 0.11 → 0.13` in bindgen; `windows-link 0.1.3 → 0.2.1` in generator. The brief allows only tauri-nspanel's own churn, so I took main's lock, removed the nine packages, inserted the resolver's own `tauri-nspanel 2.1.1` entry, and dropped the `core-graphics` suffix. Cargo accepts that lock with `--locked`. That rejected resolve is kept at `/tmp/174/Cargo.lock.update-precise`. **Risk:** the next unrelated `cargo update` will make those edge moves anyway. They come from the resolver, not from this change.
- **`dealloc` override dropped.** 2.1's macro has no hook for it, and the old override skipped `NSWindow`/`NSPanel` teardown by calling `NSObject`'s `dealloc`. Neither panel is ever closed (both are concealed, never ordered out), so this path does not run in theuth.
- **Tracking area kept** (`with: tracking_area { options: TRACKING, auto_resize: false }`). It was part of 2.0's `from_window` on every panel, so dropping it could change cursor or hover behaviour. `auto_resize: false` keeps 2.0's fixed rect: `true` would add `InVisibleRect`, which 2.0 never had.
- **Content view autoresizing mask not re-set.** 2.1 sets it on subviews only. `NSWindow` sizes its `contentView` to the content rect itself, so the mask on the content view has no effect [INFERENCE from AppKit's documented `contentView` sizing, not observed].
- **`PanelSpec` lives in `island/macos.rs`** and is shared with `point/macos.rs` through `crate::island::{PanelSpec, TRACKING}`. That avoids a new file outside the brief's list. `can_become_key_window` is a `PanelSpec` field that feeds the macro's `config`, so the spec test pins it too.
- **Concealed and revealed alpha / ignore-mouse values stay inline.** They are runtime state toggled per call, not one-time setup. Their values and order are unchanged (parity table).
- **Each `tauri_panel!` sits in its own `mod class`.** The macro expands item-level `use`s (`NSNotification`, `NSRect`, …) that would clash with `island/macos.rs`'s own imports.
- **ObjC class names** `TheuthIslandPanel` / `TheuthOverlayPanel` replace the shared `RawNSPanel`. Nothing in the repo refers to the old name.
- **`set_style_mask` errors now propagate** (`context("set_style_mask")`) out of `island::init` / `panelize`. Under 2.0, an AppKit exception there aborted the process.
- **Behaviour 2.1 adds on purpose:** `_setPreventsActivation:` sync (non-activating panels really stop activating the app) and `-windowLevel` on the WKWebView class (IME candidates above the island). Neither is a new theuth feature: both are part of `to_panel` / `set_style_mask` in 2.1.

## Open items

- **Live check (Sky's).** Nothing was launched here, per the brief.
- **New mouse overrides.** The new class implements `mouseEntered:` / `mouseExited:` / `mouseMoved:` / `cursorUpdate:` and forwards each to the window delegate only if it responds. Whether tao's delegate responds is unverified [INFERENCE: it does not, so these are no-ops]. The cursor over the island is part of the checklist below.
- **Lock edge drift** (see "Hand-pruned lock" above) will appear in the next `cargo update`.

## Sky's live checklist (about two minutes)

Build and run the desktop app as usual, then:

1. **Meetings island**
   - Show it (start a meeting or use the island toggle). It appears on the current Space. Switch to a full-screen app (for example a full-screen browser): it is still there, above it.
   - Type in another app (Notes), then click an island button. Keystrokes keep going to Notes and the menu bar still names Notes.
   - Click the request box and type. Text lands in the box. Click back into Notes: typing goes to Notes again.
   - Drag an island edge: it resizes. The cursor over the island looks normal.
   - Hide it, then show it again (conceal/reveal): it disappears fully, clicks where it was reach the app below, and it comes back at the same place.
   - Open a menu from the menu bar: the island stays above the menu.
2. **Point-and-say overlay:** hold the voice key and move the mouse (or draw with `annotate = mouse`). The trail draws over everything, including over the island and on another Space or a full-screen app. Clicks pass through to the app below, except while drawing with `annotate = mouse`. Release: the overlay disappears and clicks pass through again.
3. **⌘Tab and Mission Control:** neither the island nor an overlay appears in the ⌘Tab switcher or as a window in Mission Control.
4. **Input methods** (only if you use an IME): switch to it, click the island request box and type. The candidate window appears above the island, not behind it.
