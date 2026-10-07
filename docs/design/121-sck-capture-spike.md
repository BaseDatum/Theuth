# 121 — In-process macOS screenshots via ScreenCaptureKit (spike report)

Status: prototype spike, 2026-10-07, branch `spike/121-sck-capture` (cut from
main ebac3603). Nothing merged. The branch carries a working
`THEUTH_CAPTURE_BACKEND=sck` path in `crates/computer/src/screen.rs`, a bench
example, and this report; default behaviour is unchanged. Every external fact
below has a source; claims marked **(unverified)** were not checked against
source, docs or a run during this spike. All measurements are from one machine
(Apple Silicon, 16 cores, macOS 26.6.2, CLT 26.5, Swift 6.3.2, one 2560×1440
display at scale 1, Screen Recording granted to the terminal).

## 0. Answer

**Adopt with conditions.** In-process `SCScreenshotManager` halves the per-display
grab: **57.6 ms median vs 136.2 ms** for the `/usr/sbin/screencapture` shell-out
on a quiet machine (2.4× at median, 2.8× at p95; the ratio held at 2.0–4.0× in
two further runs under heavy external load), it needs no temp file, and the
`capture(None)` composite went **83 ms vs 163 ms**. It works from a plain
`std::thread` with no run loop (the bridge awaits on Swift's global executor), so
the `spawn_blocking` call site stays as is. The conditions are the build chain,
not the API: (1) the crate stack adds **~63–76 s** of Swift bridge compilation to
every cold `cargo build` *and* `cargo check` of `computer`, on every macOS build
host, and needs `swift` + a ≥13.0 SDK there; (2) as published, the three Swift
bridges **do not link on a Command Line Tools-only Mac** (`apple-metal` 0.10
hard-codes the full-Xcode toolchain path) and the resulting `t` binary **aborts
at launch** (`Library not loaded: @rpath/libswift_Concurrency.dylib`, "no
LC_RPATH's found") because Cargo never propagates the bridges' rpath link-arg to
dependents — both are fixed by a 25-line `build.rs` in `computer` plus one
rpath flag in `theuth`, and both should be filed upstream before landing;
(3) peak RSS of the 20-shot loop is **345 MB vs 31 MB** (phys footprint 36 MB vs
19 MB), growing sub-linearly with shot count. Binary size delta is +1.07 MB
(+0.14 %). If the build-chain cost is not acceptable on CI, do not adopt: the
shell-out is correct and 136 ms is not the bottleneck of a model turn.

## 1. The seams today (read from the tree)

| What | Where | Notes |
|---|---|---|
| Display list | `crates/computer/src/screen.rs:20-43` `enumerate()` → `xcap::Monitor::all()`; `Display { index, id, name, main, x, y, width, height, scale }` in `frame.rs:12-31`, logical points; `id` = CGDirectDisplayID (xcap-0.9.8 `macos/impl_monitor.rs:129`) | Sorted main first then `(x, y)`, `index` 1-based. SCK's `SCDisplay::display_id()` is the same CGDirectDisplayID, so the lookup is a direct id match (`screen.rs:253-258`). |
| Composite | `screen.rs:118-153` `capture(display, max_edge, tmp_dir)`: per-display `grab`, resize to the frame's logical rect (`FilterType::Triangle`), paste on a black canvas, PNG | Resizes whatever `grab` returns, so a physical-pixel grab is fine. |
| Native grab | `screen.rs:158-168` `capture_display_native(d, monitor, tmp_dir)` for `screen_context` (96 §6.4) | Wants one display at physical pixels. |
| macOS grab (before) | `screen.rs:170-216` (now `grab_screencapture`, `:189-232`): `/usr/sbin/screencapture -x -R x,y,w,h <tmp>/<pid>-<n>.png`, 5 s `try_wait` loop, `image::open`, delete | `-x` = no sound; cursor is only captured with `-C` (`man screencapture`), so the cursor is absent. Needs the temp dir created. |
| Call site | `crates/computer/src/tools.rs` runs `capture` under tokio `spawn_blocking` | Non-main thread, no run loop. |
| Permission | `screen.rs:47-56` `CGPreflightScreenCaptureAccess`; `request_screen_recording()` prompts once | Same TCC service (ScreenCapture) SCK uses. |
| Why the shell-out | `docs/design/27-computer.md` §0.4 item 4: xcap's `CGWindowListCreateImage` is deprecated and stalled 30 s in oh-my-pi; `screencapture` measured < 0.5 s; "a native platform command with a 5 s deadline beats a deprecated API"; §3.2 L220-224 spells out the command | SCK is the non-deprecated replacement Apple names for `CGWindowListCreateImage`. |
| App floor | `crates/desktop/tauri.macos.conf.json:41` `minimumSystemVersion: 15.0` | `SCScreenshotManager` is macOS 14.0+, so no floor change. |
| Build/sign chain | `scripts/package-macos.sh`: `cargo build --release -p theuth` on the host arch, codesign inside-out with `--options runtime`, `entitlements.plist` (JIT + audio-input), notarytool via `just notarize-macos` | No Swift anywhere in the tree before this spike; `.cargo/config.toml` has no macOS rustflags. |

## 2. Crate facts

Read 2026-10-07 from the crates.io API, the GitHub API and the fetched source
(`~/.cargo/registry/src/*/screencapturekit-11.0.0`).

| Fact | Value | Source |
|---|---|---|
| Crate | `screencapturekit` 11.0.0, published 2026-09-24; MSRV 1.82, edition 2021 | `Cargo.toml` of the fetched crate; crates.io `versions[]` |
| License | MIT OR Apache-2.0 (GitHub shows Apache-2.0 only because it detects one file) | `Cargo.toml` `license`; GitHub API `license.spdx_id` |
| Author / maintainer | Per Johansson (`1313`), 648 of ~690 commits; next contributors are Copilot (24), bots, one human with 5 | GitHub contributors API |
| Maintenance signal | 242 ★, 44 forks, pushed 2026-09-24, **0 open issues, 0 open PRs**, 35 closed issues total; releases 9.0.1 (08-31) → 10.0.0/.1/.2/.3 (all 09-07) → 11.0.0 (09-24): three majors in four weeks | GitHub repo/releases/search API |
| Downloads | 2.53 M total, 1.73 M in 90 days, 22 reverse dependencies on crates.io | crates.io API |
| Dependencies | `apple-cf` 0.11.0 (features cg, iosurface, dispatch, cv, cm), `apple-metal` 0.10.0, `doom-fish-utils` 0.4.1, `crossbeam-queue`, `futures-util`, `libc`. Same author; `apple-cf` and `apple-metal` each ship their own Swift bridge and build.rs | `cargo fetch` + `Cargo.lock` diff (40 lines, 4 new packages) |
| Not objc2 | Its `CGImage` is `apple_cf::cg::CGImage` (re-exported). No shared types with the `objc2-*` crates already in `computer` | `src/screenshot_manager.rs:55` |
| Swift bridge | `build.rs:186-291`: `cargo:rustc-link-lib=framework=ScreenCaptureKit`; `swift build -c release --triple arm64-apple-macosx13.0 --package-path swift-bridge --scratch-path $OUT_DIR/swift-build-<arch>-sdk<maj>-<min>-<variant>` with `-Xswiftc -DSCREENCAPTUREKIT_HAS_MACOS14_SDK` … per enabled feature; links `libScreenCaptureKitBridge.a` static plus Foundation/CoreGraphics/CoreMedia/IOSurface; adds `-Wl,-rpath,/usr/lib/swift` and `<xcode-select -p>/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx` as link-args; skipped under `DOCS_RS`. Requires the macOS 13.0+ SDK via `xcrun --sdk macosx --show-sdk-version` (asserts). | `build.rs` |
| Bridge deployment target | screencapturekit bridge 13.0 (`Package.swift` `.macOS(.v13)`, triple pinned); apple-cf bridge 13.0; **apple-metal bridge 11.0** (`.macOS(.v11)`, triple `arm64-apple-macosx` without a version) — that is what drags in the Swift back-deployment compatibility libraries (§6) | the three `swift-bridge/Package.swift` and `build.rs` files |
| Features | Cumulative `macos_13_0 … macos_26_0`; `macos_14_0` enables `SCScreenshotManager`; `macos_15_2` adds `capture_image_in_rect`; `macos_26_0` adds `SCScreenshotConfiguration` (HDR). This spike uses `macos_15_0` (the app's floor); **`macos_14_0` would suffice** for screenshots | `Cargo.toml` `[features]`; `src/screenshot_manager.rs` cfgs |
| Sync API | `SCScreenshotManager::capture_image(&filter, &config) -> Result<CGImage, SCError>` blocks on a `SyncCompletion` with a **30 s default timeout** (`SCREENCAPTUREKIT_COMPLETION_TIMEOUT_SECS` overrides, `0` disables); the Swift side runs `Task { try await SCScreenshotManager.captureImage(...) }` — no run loop, no main thread | `src/screenshot_manager.rs:570-587`, `src/utils/completion.rs:21-48`, `swift-bridge/.../ScreenshotManager.swift:11-40` |
| Pixel access | `CGImageExt::rgba_data()` renders through CoreGraphics into a tightly packed `w*4` buffer (`required_byte_size = w*h*4`), so there is no stride to handle; `rgba_data_into_strided` exists if we ever want our own buffer | `src/screenshot_manager.rs:430-530` |
| Content enumeration | `SCShareableContent::get()` (all windows) and `SCShareableContent::create().with_on_screen_windows_only(true).with_exclude_desktop_windows(true).get()`; `SCShareableContent::current_process()` (14.4+) is TCC-free but returns only our own windows, useless for a display grab | `src/shareable_content/mod.rs:164-186, 362-409, 466-513` |
| Error surface | `SCError::NoShareableContent(String)` / `ScreenshotError(String)` carry the bridge's `"Content unavailable: …"` / `"Screenshot error: …"` + `error.localizedDescription`; `SCError::PermissionDenied` exists but the screenshot path never produces it — denial arrives as text | `src/utils/error.rs:95-239`, `swift-bridge/.../Core.swift:108-140` |
| Linting profile | Crate denies nothing but warns pedantic/nursery/cargo; our workspace `clippy::all = deny` passed with it as a dependency | `Cargo.toml` `[lints]` |

## 3. What was built

All on the branch, default behaviour unchanged; `cargo test -p computer` (43
tests) and `cargo clippy -p computer --all-targets -- -D warnings` green.

| File | Lines | What |
|---|---|---|
| `/Cargo.toml` | 294-297 | `screencapturekit = { version = "11", default-features = false, features = ["macos_15_0"] }` with the one-line-comment style of the block. |
| `/Cargo.lock` | +40 | Only `screencapturekit`, `apple-cf`, `apple-metal`, `doom-fish-utils` and their edges; no `cargo update`. |
| `crates/computer/Cargo.toml` | 28-29 | `screencapturekit = { workspace = true }` under the macOS target table. |
| `crates/computer/build.rs` | 1-41 (new) | **Needed to link and to run on this machine** (§6): resolves `xcrun --find swift` → `<toolchain>/usr/lib/swift/macosx` and emits it as `rustc-link-search` (the static `libswiftCompatibility*.a` live there on a CLT-only host), and emits `-Wl,-rpath,/usr/lib/swift` so the crate's tests and examples launch. The rpath only reaches `computer`'s own binaries; see §6 for `theuth`. |
| `crates/computer/src/screen.rs` | 170-183 | `grab()` dispatch: `THEUTH_CAPTURE_BACKEND=sck` → `grab_sck`, else `grab_screencapture` (the unchanged shell-out, renamed, `#[doc(hidden)] pub` so the bench can call it). |
| | 241-284 | `grab_sck(d)`: on-screen-only `SCShareableContent`, `SCDisplay` by `display_id == d.id`, `SCContentFilter::create().with_display(..).with_excluding_windows(&[])`, `SCStreamConfiguration` with `width/height = round(logical × scale)` and `shows_cursor(false)`, `capture_image`, `rgba_data` → `RgbaImage::from_raw`. Errors → `CaptureError::Other`, or `NoScreenRecording` when `screen_recording_granted()` is false. No caching across calls. |
| | 290-297, 301-306 | `sck_shareable_content()` (the cheaper getter, chosen after measuring both, §4.2) and `sck_pixel_size()`. |
| | 308-328 | `sck_tests::grab_sck_works_off_main_thread`: runs `grab_sck` on a `std::thread::spawn` thread and asserts the physical dimensions; skips with a message when Screen Recording is not granted. **Passes: 0.19 s.** |
| `crates/computer/examples/capture_bench.rs` | 1-272 (new) | macOS-only (stub `main` elsewhere). Per display and backend: 3 warm-ups + 20 timed grabs → min/median/p95, dimensions, raw bytes; `SCShareableContent::get()` vs on-screen-only getter; `capture_image` alone; `rgba_data` alone; filter build alone; the uncached-`get()` composition; the cached-filter composition; back-to-back cross-backend diff (equal dimensions? mean |ΔRGB| per channel); `capture(None, 1568)` under each backend; `--save <dir>` writes `shot-<backend>-<n>.png` + `composite-<backend>.png`; `--loop <backend> [--rounds N]` for `/usr/bin/time -l`. |

Not built: nothing from the brief was skipped. The HDR path (`macos_26_0`)
and `capture_image_in_rect` (`macos_15_2`) were read but not exercised.

## 4. Measurements

Release build of the example (`cargo build -p computer --example capture_bench --release`),
one display 2560×1440 @ scale 1 (the machine has no Retina panel attached, so
"physical = logical" here; `sck_pixel_size` multiplies by `scale` and the
screencapture path was checked to return 2560×1440 for the `-R` of the full
display — both backends agree on dimensions). 3 warm-ups, 20 timed runs. Saved
PNGs: `/tmp/sck-spike/shot-sck-1.png`, `shot-screencapture-1.png`,
`composite-sck.png`, `composite-screencapture.png`.

### 4.1 Latency per backend per display (wall-clock ms)

Three runs. Run 1 was the quietest; during runs 2 and 3 other sessions were
compiling in parallel worktrees (load average 25–50 on 16 cores), which is why
every row moved. The ratio is the stable number.

| Display #1, 2560×1440, 14 745 600 raw bytes | Run 1 min / median / p95 | Run 2 | Run 3 (load avg 25–50) |
|---|---|---|---|
| **screencapture** (spawn + wait + PNG decode + unlink) | **120.1 / 136.2 / 174.5** | 146.0 / 159.1 / 171.9 | 235.8 / 407.6 / 582.4 |
| **sck** (`grab_sck`: on-screen-only content + filter + capture + RGBA) | **53.2 / 57.6 / 61.5** | 67.7 / 81.0 / 94.6 | 91.3 / 103.0 / 140.5 |
| ratio screencapture ÷ sck at median | **2.4×** | 2.0× | 4.0× |
| `capture(None, 1568)` composite, one shot | 163.1 vs **83.2** | 227.9 vs 142.0 | 430.3 vs 183.1 |

PNG of the composite: 361 792 B (screencapture) vs 344 477 B (sck), same
1568×882 frame.

### 4.2 Where SCK's time goes (ms, same runs)

| Piece | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| `SCShareableContent::get()` (displays + every window + apps) | 26.3 / 27.8 / 28.8 | 27.9 / 31.8 / 36.9 | 44.1 / 51.5 / 61.2 |
| on-screen-only, exclude-desktop getter (what `grab_sck` uses) | 12.8 / 13.8 / 15.4 | 15.8 / 16.7 / 17.2 | 20.3 / 24.2 / 26.1 |
| `SCContentFilter` build | — | 0.0 | 0.0 |
| `SCScreenshotManager::capture_image` alone (filter + config prebuilt) | 26.3 / 29.1 / 30.9 | 34.0 / 41.3 / 42.7 | 48.9 / 56.0 / 62.4 |
| `CGImage → RGBA` (`rgba_data`, 14.7 MB) | 0.2 / 0.2 / 0.2 | 0.3 / 0.3 / 0.4 | 0.4 / 0.4 / 0.6 |
| composed: full `get()` every call (the brief's uncached v1) | — | 65.8 / 81.9 / 94.2 | 135.2 / 155.9 / 200.5 |
| composed: cached `SCDisplay`/filter, capture + RGBA only | — | 40.0 / 42.7 / 45.9 | 63.0 / 90.5 / 141.1 |

Reading: enumeration is 25–50 % of the SCK grab. The on-screen-only getter is
half the cost of `get()`, so `grab_sck` uses it (both numbers reported as the
brief asked). Caching the `SCDisplay`/filter for the process lifetime would
remove the remaining 14–24 ms per call at the price of a refresh-on-miss path
when displays change; not done in the spike. `rgba_data` is effectively a
memcpy (0.2–0.4 ms for 14.7 MB), so the "stride" concern in the brief does not
arise: the bridge renders into our tightly packed buffer.

### 4.3 Peak memory of the 20-shot loop (`/usr/bin/time -l`, release)

| Backend | wall | maximum resident set size | peak memory footprint |
|---|---|---|---|
| screencapture | 2.99–8.59 s | **31.0 MB** | 19.3 MB |
| sck | 1.38–2.83 s | **345 MB** | 36.5 MB |

Scaling check (`--rounds`): sck RSS 63.6 MB (1 shot) → 123 MB (5) → 345 MB (20)
→ 522 MB (60) while the footprint stays 35.2–36.8 MB; screencapture stays at
31 MB / 18.6–19.0 MB throughout. The RSS growth is sub-linear and the footprint
flat, so the pages are freed-but-still-mapped (IOSurface/CoreGraphics backing
of the released `CGImage`s, or allocator retention) rather than a per-shot leak
**(cause unverified; it is what `/usr/bin/time` reports)**. The daemon takes one
or two shots per model turn, not 20 in 3 s, but a long session should be checked
with `footprint`/`leaks` before landing.

### 4.4 Build cost

| What | Measured |
|---|---|
| Cold `cargo build -p computer` after `cargo clean -p screencapturekit -p apple-cf -p apple-metal` (dev profile; only those three crates + `computer` rebuilt) | **76 s wall** (user 99 s) |
| Cold `cargo check -p computer` after the same clean | **63 s wall** (user 91 s) — build scripts run under `check`, so **check pays the Swift bridges too** (a first check that also had other crates to check took 79 s) |
| Each bridge alone, `swift build -c release` with the exact build.rs arguments, sequential | apple-metal 26.5 s · apple-cf 27.3 s · screencapturekit 35.7 s (= 89.5 s serial; cargo overlaps them to the 63–76 s above) |
| Static libs produced under `target/debug/build/*/out/` | `libAppleCFBridge.a` 900 896 B · `libAppleMetalSwiftBridge.a` 2 078 792 B · `libScreenCaptureKitBridge.a` 1 788 176 B (4.77 MB) |
| Warm incremental rebuild of `computer` after a `screen.rs` edit | 3.4–4.4 s (bridges cached in `OUT_DIR`) |
| `cargo build -p theuth --release`, cold workspace | 18 m 48 s (this machine, loaded); not attributable to SCK |
| `t` binary size, release: baseline (dep stashed) vs with SCK | 735 431 456 B → 736 497 520 B = **+1 066 064 B (+0.14 %)**; `__TEXT` +311 KB; 0 → 23 `/usr/lib/swift/libswift*.dylib` load commands (all system, 14 weak) |

### 4.5 Pixel diff (back-to-back shots, still desktop)

Dimensions equal (2560×1440) in every run. Mean |ΔRGB| per channel:
**1.468** (run 1), 1.484, 1.486. Breakdown of run 1 (`uv run --with pillow --with numpy`):

- 99.9 % of pixels differ somewhere; histogram of channel deltas: 0 → 6.95 M,
  1 → 3.56 M, 2 → 169 k, ≥ 3 → the rest. Body of the screen excluding the menu
  bar and a 2-pixel border: **1.09**. This is colour management: `screencapture`
  writes the display's profile (`sips -g profile raw.png` → "LG ULTRAGEAR+") and
  `image::open` keeps those bytes; SCK's `CGImage` is rendered into an sRGB
  context by the bridge **(sRGB target inferred from the ±1 pattern, not read
  from the Swift source)**.
- Menu bar rows 0–40: **13.5**. Both backends include the menu bar; the
  translucent bar's blur/tint renders lighter in SCK (crop
  `/tmp/sck-spike/crop-menubar-right.png`). Glyphs and the clock are identical.
- Outer 2-pixel border: rows 1-2 ≈ 7, rows 1437-1438 ≈ 25, columns 1-2 and
  2557-2558 ≈ 17–18. The frontmost window's edge highlight is a 1-px grey line
  in `screencapture` and a bright white line in SCK, and the wallpaper strip
  outside it reads (9,21,42) vs (1,44,91) (`crop-bottom-right.png`): SCK
  composites window layers itself while `screencapture` reads the final
  framebuffer, so edge effects of the window server differ **(mechanism
  unverified)**.
- One text line in a scrolling terminal changed between the two shots (real
  content change, visible in `diff-x16.png`).

## 5. Behavioural differences

| Topic | screencapture path | SCK path (observed unless marked) |
|---|---|---|
| Cursor | Absent (`-C` not passed) | Absent: `with_shows_cursor(false)`; SCK's default is `true` (`captured_elements.rs:31`), so the flag matters. |
| Menu bar | Included for `-R` of the whole display | Included. Apple's default for display-including filters is `includeMenuBar = false` per the crate docs (`content_filter.rs:594-596`), yet the shots show it — the display+excludingWindows filter is a display-*excluding* constructor in Apple's terms, default `true`. Rendered with a visibly different translucency (§4.5). `with_include_menu_bar(false)` is available (macOS 14.2+) if we ever want it out. |
| Notch | Not testable here (no notched display). `screencapture` of a notched display returns the full panel incl. the notch area; SCK returns the display's `frame` at `width × height` points, which on notched Macs also covers the full panel **(unverified)**. | |
| Windows with `sharingType = .none` | Hidden by the window server from both paths. Apple now documents `NSWindowSharingNone` as a legacy constant macOS no longer uses and says not to rely on it ([AppKit docs](https://developer.apple.com/documentation/appkit/nswindow/sharingtype-swift.enum/none)); a 2026 forum thread reports SCK display captures that first omitted and then included such an app ([forums thread 808016](https://developer.apple.com/forums/thread/808016), FB21115847). Not exercised. |
| HDR displays | 8-bit SDR PNG | `capture_image` returns an SDR `CGImage`; HDR needs `SCScreenshotConfiguration` + `capture_screenshot` behind `macos_26_0` ([Apple: SCScreenshotManager](https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager)). Our model input is SDR anyway. Not exercised (no HDR panel). |
| Multiple displays | One `screencapture` process per display, sequential | One `SCShareableContent` + one `capture_image` per display, sequential; the content list could be fetched once per `capture()` call (not done). `capture(None)` composite produced the same 1568×882 frame under both backends. Only one display was attached, so the multi-display path is exercised only through the same code with `n = 1`. |
| Screen Recording denied | `screencapture` exits non-zero → `NoScreenRecording` (27 §0.4) | Not revoked in this spike. From source: `SCShareableContent` fails with `SCStreamErrorUserDeclined` (-3801, "the user didn't grant Screen Recording permission" — [Apple error constants](https://developer.apple.com/documentation/screencapturekit/scstreamerror/code/userdeclined)); the bridge formats it as `"Content unavailable: <localizedDescription>"` → `SCError::NoShareableContent`; `grab_sck` maps any `SCError` to `NoScreenRecording` when `CGPreflightScreenCaptureAccess` is false, else `Other`. **Exact text unobserved.** The 30 s completion timeout (§2) is the worst case if the framework never calls back; the shell-out's is 5 s. Worth setting `SCREENCAPTUREKIT_COMPLETION_TIMEOUT_SECS=5` or wrapping in our own deadline before landing. |
| Temp files | Writes + deletes `<tmp>/<pid>-<n>.png` | None. |
| Colour | Display profile bytes, untagged after re-encode | sRGB-rendered (§4.5). For a model input this is a wash; for pixel-exact comparisons it is a 1-level shift. |
| Threading | Child process | Any thread; proven by `grab_sck_works_off_main_thread` and by the bench running on the main thread of a plain binary without a run loop. |

## 6. Build-chain and distribution risks

1. **Swift toolchain on every macOS build host.** `swift build` runs in three
   build scripts (`screencapturekit`, `apple-cf`, `apple-metal`), ~63–76 s cold,
   also under `cargo check`, also under `cargo clippy`. Needs a macOS ≥ 13.0
   SDK (`xcrun --sdk macosx --show-sdk-version`, asserted in `build.rs:209`) and
   `swift` on PATH. Our Linux/Windows hosts are unaffected (target-gated dep),
   but any macOS CI runner, `cargo check` in editors, and `rust-analyzer`'s
   check on save all pay it once per `OUT_DIR`. Measured, not estimated.
2. **CLT-only hosts do not link as published.** First link attempt here failed:
   `ld: Could not find or use auto-linked library 'swiftCompatibility56'` /
   `'swiftCompatibilityConcurrency'` / `'swiftCompatibilityPacks'`, then
   `Undefined symbols: __swift_FORCE_LOAD_$_swiftCompatibility56 … referenced
   from AppleMetalSwiftBridge in libapple_metal-….rlib`. Cause: `apple-metal`
   0.10's `build.rs:71-79` adds `rustc-link-search` only for
   `<xcode-select -p>/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx`,
   which exists under full Xcode and not under
   `/Library/Developer/CommandLineTools` (the libs are at
   `/Library/Developer/CommandLineTools/usr/lib/swift/macosx/`). The force-loads
   come from `apple-metal`'s bridge targeting macOS 11 (`.macOS(.v11)`, triple
   without version), which makes Swift emit back-deployment compatibility
   references; the other two bridges target 13.0 and need none. Fixed in this
   spike by `crates/computer/build.rs` (derive the directory from
   `xcrun --find swift`). Upstream fix: same derivation in `apple-metal`, or
   pin its triple to `macosx13.0` like its siblings.
3. **The shipped binary aborts at launch without an rpath.** The three
   bridges emit `cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift`, but Cargo
   applies `rustc-link-arg` only to the emitting package's own binaries, never
   to dependents, so the first `computer` test binary and then the release `t`
   had **no `LC_RPATH` at all** (`otool -l`) while linking
   `@rpath/libswift_Concurrency.dylib` (the toolchain's back-deployment copy,
   found through the search path from item 2). Observed:
   `dyld[1909]: Library not loaded: @rpath/libswift_Concurrency.dylib …
   Reason: no LC_RPATH's found` from `target/release/t --version`. With
   `install_name_tool -add_rpath /usr/lib/swift` on a copy, `t 2.9.2` runs.
   `computer/build.rs` adds the rpath for `computer`'s tests/examples; `theuth`
   (and any other bin that links `computer`) needs the same flag — a one-line
   `build.rs` in `crates/theuth`, or `[target.aarch64-apple-darwin] rustflags =
   ["-C", "link-arg=-Wl,-rpath,/usr/lib/swift"]` in `.cargo/config.toml`. Both
   outside this spike's allowed file set; not done.
4. **Swift runtime at run time.** Every other Swift library resolves to
   `/usr/lib/swift/libswift*.dylib` (23 load commands, 14 weak), which macOS has
   shipped in the dyld shared cache since 10.14.4 and `libswift_Concurrency`
   since 12.0 **(Apple's ABI-stability statement; not re-verified here)**; our
   floor is 15.0. So no runtime dylibs need bundling once the rpath exists; the
   bridges are static.
5. **x86_64.** `build.rs` passes `--triple x86_64-apple-macosx13.0` when
   `CARGO_CFG_TARGET_ARCH = x86_64`, so cross-building from Apple Silicon is
   designed for; `scripts/package-macos.sh` builds the host arch only. Not
   exercised.
6. **Signing / notarization.** The static bridges become part of `t`; nothing
   new to sign. The hardened runtime does not restrict ScreenCaptureKit, and
   the app's TCC prompt is the same Screen Recording grant the shell-out
   already needs. **`NSScreenCaptureUsageDescription`** is listed by Apple for
   ScreenCaptureKit apps ([ScreenCaptureKit docs](https://developer.apple.com/documentation/ScreenCaptureKit));
   `crates/desktop/Info.plist` should be checked for it before landing
   **(whether its absence blocks anything for a CLI sidecar: unverified)**.
7. **Binary size.** +1.07 MB on a 735 MB `t` (+0.14 %).
8. **Crate churn.** Three major versions in four weeks (9 → 10 → 11), one
   maintainer, zero open issues. Pin `=11.0.x`; expect API moves on upgrade.
9. **Toolchain drift.** `swift` 6.3.2 built it; the bridges use
   `swift-tools-version:5.9`. A future Xcode/CLT update can break the Swift
   build on CI with no Rust change (the crate's `build.rs:160-169` already has a
   "stub mode" warning path for SDK/feature mismatches).

## 7. What landing would need

1. `crates/theuth/build.rs` (or `.cargo/config.toml` rustflags) adding
   `-Wl,-rpath,/usr/lib/swift`; keep `crates/computer/build.rs` until
   `apple-metal` fixes its search path upstream (file both: apple-metal link
   search on CLT hosts; propagate-rpath documentation in screencapturekit).
2. Make SCK the default in `grab()` and drop the env switch, or keep
   `THEUTH_CAPTURE_BACKEND` as a documented fallback for one release.
3. A deadline: `SCREENCAPTUREKIT_COMPLETION_TIMEOUT_SECS` or our own 5 s wrap,
   so the worst case matches today's.
4. Fetch `SCShareableContent` once per `capture()` call (not per display), and
   decide on caching the `SCDisplay` list with refresh-on-miss (§4.2, saves
   14–24 ms per shot).
5. A long-session memory check (`footprint`, `leaks`) for the RSS growth in §4.3.
6. Test on a Retina + notched display and a two-display setup; compare
   `screen_context`'s native grab (`capture_display_native`) dimensions.
7. Add `swift` + CLT/Xcode to the macOS CI image notes; budget ~70 s per cold
   check/build; consider caching `target/*/build/*/out/swift-build*`.
8. Design 27 edits:
   - §0.4 item 4 "Capture: `/usr/sbin/screencapture -x -R x,y,w,h` per display
     on macOS" → "Capture: ScreenCaptureKit `SCScreenshotManager` in-process per
     display on macOS (121)"; keep the sentence about xcap's deprecated
     `CGWindowListCreateImage` as the reason xcap is enumeration-only, and
     replace "a native platform command with a 5 s deadline beats a deprecated
     API" with "the non-deprecated framework, 2× faster than the shell-out, with
     the same 5 s deadline (121 §4)".
   - §3.2 L220-224 (the `screencapture -x -R…` command, the `try_wait` loop,
     "decode with `image`") → the SCK sequence of 121 §3; L394-396
     "`<data_dir>/computer/tmp/` — `screencapture` output, deleted after
     decode" → delete (no temp files).
   - §4 facts row "`/usr/sbin/screencapture`: `-x` … `-R` …" (L107-110) →
     the SCK facts of 121 §2.
   - Dependency table row (L486-487) gains `screencapturekit 11 (121) + Swift
     bridge build`.
   - `crates/computer/src/screen.rs` module doc lines 1-5 and the `tmp_dir`
     parameter of `capture` / `capture_display_native` (becomes unused on
     macOS; off-macOS it already is).

## 8. Open questions

- Is a ~70 s Swift step on every cold macOS `cargo check` acceptable for the
  people who build on Macs daily? That is the real cost; the capture itself is
  a clear win.
- Does `apple-metal` even need to be in the tree? `screencapturekit` depends on
  it unconditionally for its Metal texture paths; a feature to drop it would
  remove one bridge, the macOS 11 back-deployment libs, and the CLT link bug.
  Worth asking upstream.
- The RSS behaviour (§4.3): allocator retention or an IOSurface cache? Decide
  with `leaks` on a 10-minute session before defaulting.
- Should the menu bar be excluded now that there is a switch (`with_include_menu_bar(false)`)? Today's shell-out includes it, and the model
  uses the clock/status area occasionally; keep it unless it misleads.
- Colour: should we tag the PNG as sRGB (SCK) rather than leave it untagged?
  Irrelevant to the model; relevant if screenshots are ever diffed.
- A Retina/notch run is still owed; the physical-pixel assumption
  (`round(logical × scale)`) is checked only at scale 1 here.

## 9. Sources

- Crate: https://github.com/doom-fish/screencapturekit-rs · https://docs.rs/screencapturekit/11.0.0 · https://doom-fish.github.io/screencapturekit-rs/screencapturekit/ · crates.io API `/api/v1/crates/screencapturekit` (+ `/reverse_dependencies`), `apple-cf`, `apple-metal` · GitHub API repo / releases / contributors / search issues — all read 2026-10-07.
- Fetched source read: `~/.cargo/registry/src/*/screencapturekit-11.0.0/{build.rs,Cargo.toml,CHANGELOG.md,src/screenshot_manager.rs,src/shareable_content/mod.rs,src/stream/content_filter.rs,src/stream/configuration/*.rs,src/utils/{completion,error}.rs,swift-bridge/Package.swift,swift-bridge/Sources/ScreenCaptureKitBridge/{ScreenshotManager,Core}.swift}`; `apple-metal-0.10.0/{build.rs,swift-bridge/Package.swift}`; `apple-cf-0.11.0/{build.rs,swift-bridge/Package.swift}`.
- Apple: https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager · https://developer.apple.com/documentation/screencapturekit/scstreamerror/code/userdeclined · https://developer.apple.com/documentation/appkit/nswindow/sharingtype-swift.enum/none · https://developer.apple.com/forums/thread/808016 · https://developer.apple.com/documentation/ScreenCaptureKit (usage-description key) · `man screencapture` (26.6.2).
- Tree: `crates/computer/src/{screen,frame,tools}.rs`, `crates/computer/Cargo.toml`, `/Cargo.toml`, `docs/design/27-computer.md`, `docs/design/78-aec-spike.md`, `crates/desktop/{tauri.macos.conf.json,entitlements.plist}`, `scripts/package-macos.sh`, `.cargo/config.toml`.
- Raw outputs: `/tmp/sck-spike/bench-release.txt` (run 2), `bench-release-run3.txt`, `rss.txt`, `rss-scaling.txt`, `swift-*.log`, the PNGs and crops listed in §4.5; run 1 numbers are in this document only (its stdout was captured before the bench grew the two composed rows).
