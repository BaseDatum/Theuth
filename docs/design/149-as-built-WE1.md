# 149 — as-built WE1: the Windows UI Automation reader for `screen_context`

Branch `feat/149w-we1` off `integrate/149` 77fa225f (= main dd19bf63 + the design); worktree `~/Develop/theuth-149w-we1`;
worker WE1 (theuth subagent), 2026-10-09 01:38 → 03:30 BST (R1 03:16 → 03:30). Verified on the Mac (fmt, check, tests, clippy) and on
build-B (Windows 11 Pro, the build service's `cargo-check` / `cargo-test` in slot `we1`, interactive session, one display).

## 1. What was built

Design 149 §7.4 #5 keeps capture and accessibility in the engine (`crates/computer`). Before this wave `ax.rs` was a
macOS `AXUIElement` reader and every other OS answered `granted() = false`, so the `[screen …]` line said
`accessibility: off` on Windows (96 §0.14). Now `ax.rs` has a Windows backend, `mod windows_uia`, behind
`#[cfg(target_os = "windows")]`, with the macOS reader's public surface and semantics:

| Area | File | What |
|---|---|---|
| Reader | `crates/computer/src/ax.rs` `windows_uia` | `granted()` → `true`; `Reader::hit_at(Pt)` → `IUIAutomation::ElementFromPoint`; `frontmost()` → `GetForegroundWindow` → `ElementFromHandle`; `own_bundle_id()` → `None` |
| Reader | same | **element**: `Name` → `label`, control type → `role` (`uia_role`), `ValuePattern.Value` → `value` (a Hyperlink's URL-shaped value → `url`) |
| Reader | same | **app**: the top-level window's `CurrentProcessId` (else the element's) → `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` → `QueryFullProcessImageNameW` → exe stem (`msedge`, `theuth-desktop`, `Code`) in `AxFront.app`; `bundle_id` is `None` |
| Reader | same | **window**: `ControlViewWalker` parents up to the root's child (≤ 32 steps), its `Name`, else `GetWindowTextW` on its HWND → `window_title` |
| Reader | same | **document URL** (browsers only — `is_browser`: chrome, msedge, firefox, brave, opera, vivaldi, arc, chromium): one `FindFirst(Descendants, ControlType == Document)` and its `Value` (Chromium and Firefox put the page URL there), else the first `Edit` (the address bar); accepted when `looks_like_url` |
| Reader | same | **Chromium content**: a nameless `Chrome_RenderWidgetHostHWND` pane (the tree is not built until the first UIA query switches it on) is retried 3 × 120 ms, once per process per reader — the macOS `AXManualAccessibility` shape |
| Shared | `ax.rs` | `is_self` on Windows also matches theuth's exe stems (`is_theuth_exe`: `t`, `theuth-desktop`, `Theuth.Companion`); pure helpers `physical_point`, `uia_role`, `is_browser`, `looks_like_url` compile and test on every OS; the no-reader fallback is now Linux-only; **R1**: `AxHit.refused` + `AxHit::refused()` |
| Line | `crates/computer/src/context.rs` | **R1**: `Lookups { answered, refused }` per visited display, `elements_for` returns them, `display_ax_on(ax_on, lookups)` picks the per-display suffix; two pure tests |
| Deps | `crates/computer/Cargo.toml`, `Cargo.lock` | `windows = "0.62"` under `[target.'cfg(target_os = "windows")'.dependencies]` with `Win32_Foundation`, `Win32_System_Com`, `Win32_System_Ole`, `Win32_System_Threading`, `Win32_System_Variant`, `Win32_UI_Accessibility`, `Win32_UI_WindowsAndMessaging`. 0.62.2 is already in the graph through xcap 0.9.8, so the lock gains one dependency edge (`computer` → `windows 0.62.2`), no new crate version |

### What the `[screen …]` line contains on Windows now

`context.rs` is untouched; the line's grammar is 96 §6.6 and the Windows reader fills the same fields:

```
[screen 1/1 "msedge" · "basedatum — GitHub - Personal - Microsoft Edge" · https://github.com/basedatum · pointer: circled (612,418)–(884,452): link "theuth" → https://github.com/basedatum/theuth, text "Rust"; ended on link "theuth" → https://github.com/basedatum/theuth · accessibility: on]
```

- `"msedge"` is the exe stem of the top-level window's process (not a localized app name — Windows has no
  `localizedName`; the model reads it fine and it is stable across locales).
- the title is the top-level window's UIA `Name` (`GetWindowTextW` fallback); a window with an empty title (theuth's own
  Tauri window on Windows has one) is omitted, as on macOS.
- the URL field appears for browsers only, from the page Document's `Value` (full URL with scheme), else the address
  bar's text as shown.
- element roles use the macOS vocabulary where it exists (`link`, `text`, `field`, `button`, `image`, `document`, `pane`,
  `window` …) so the prompt block (`context::PROMPT`) needs no Windows variant; Custom/unknown control types fall back to
  the localized control type squashed and lowercased, then `element`.
- `accessibility: on` whenever the request asks for it and `cfg.accessibility == auto`, **per display** (R1, 149 §9): a
  lookup the tree REFUSES — `E_ACCESSDENIED` under UIPI on an elevated window, `UIA_E_ELEMENTNOTAVAILABLE`,
  `UIA_E_TIMEOUT`, a pid read that fails on the element, or the reader's worker dead / not started / over its 2 s
  budget — comes back as `AxHit::refused()` (no element, no app, `refused: true`). `context.rs` counts answered vs refused
  lookups per display (`Lookups`); when **every** lookup on a display was refused its line says `accessibility: off`
  (`display_ax_on`); a display with at least one answered lookup — or none at all — stays `on`, and the refused anchors
  fall back to coordinates as a `None` hit does on macOS. The macOS reader never sets `refused`, so its lines are unchanged.

### Point mapping

Points reach `screen_context` as "logical global, origin at the primary display's top-left" (96 §6.3). On Windows the
displays xcap reports come from `DEVMODEW` (`dmPosition`, `dmPelsWidth/Height`): **physical pixels**, so the capture's
`Scale::of` ratio is 1 and the pointer space the engine already uses on Windows *is* the physical screen — the space
`GetCursorPos` reports under PerMonitorV2 (149 §7.4 #4) and the space `ElementFromPoint` hit-tests in. The reader maps
with `physical_point(p, origin, ratio)` (rounded to the nearest pixel, scaled from the display origin) and
`WINDOWS_CAPTURE_RATIO = 1.0`; the function is pure and tested for ratios 1, 1.5 and 2, so a future logical-geometry
source changes one constant. **Consequence for WW3:** the companion must send samples in physical screen pixels
(`GetCursorPos` from a PerMonitorV2 process) — anything else would already disagree with the capture.

### Timeouts and robustness

- COM runs on one dedicated MTA thread per `Reader` (`Worker`, thread `uia-reader`): `CoInitializeEx(MULTITHREADED)`,
  `CoCreateInstance(CUIAutomation8)` (falls back to `CUIAutomation`), `IUIAutomation2::SetConnectionTimeout(1000 ms)` and
  `SetTransactionTimeout(1000 ms)`. `compose()` runs inside `spawn_blocking`, so the calling thread blocks on a channel.
- Wall-clock bound: `HIT_BUDGET` = **2000 ms per lookup** (`recv_timeout`). On overrun the reader logs a warning, drops
  its sender and answers `None` for the rest of that `compose()`; the stuck thread exits when the provider call returns.
  So an unresponsive app costs one `screen_context` at most ~2 s of accessibility time plus up to 2 s in `frontmost()`,
  never a hang.
- `frontmost()` uses a one-shot `Worker` with the same budget.
- Every Win32/COM call is checked; `None` on any failure; no `unwrap`/`expect` outside tests. `unsafe` blocks each carry a
  SAFETY note.
- The Hyperlink `Value` is taken as the URL only when `looks_like_url` (no whitespace; a scheme, `about:`, or a dot).

## 2. Verification

Mac (`~/Develop/theuth-149w-we1`, mbx managed target, `BUILD_LABEL=149w-we1` under `build-lock`), at 350db0bf:
- `cargo fmt -p computer --check` clean (re-run at every commit; the later commits change only `cfg(windows)` lines);
- `mbx check -p computer` → Finished, rc 0 (3 m 41 s);
- `mbx test -p computer --no-run` then `mbx test -p computer` (bare, `RUST_TEST_THREADS=2`) → **50 passed, 0 failed**
  (the 46 pre-existing + `theuth_exe_stems`, `physical_point_scales_from_the_display_origin`,
  `uia_roles_use_the_line_vocabulary`, `browsers_and_urls`);
- `mbx clippy -p computer --all-targets -- -D warnings` → Finished, rc 0 (2 m 23 s).

build-B (service `cargo-check` / `cargo-test`, slot `we1`, `CARGO_TARGET_DIR C:\Users\user\ct-we1`):
- ec516c9b `cargo-check -p computer --tests` → exit 101: the pre-existing `cfg(not(macos))` fallback (`hit_at`,
  `frontmost`, `own_bundle_id`) now collided with the Windows module (6 errors, 203 s cold) — fixed in 350db0bf;
- 350db0bf `cargo-check -p computer --tests` → **0 errors, 0 warnings** (job 20261009T005956-2, 8 s warm);
- 350db0bf `cargo-test -p computer --lib` → **47 passed, 0 failed, 1 ignored** (job 20261009T010358-4, 4 m 02 s build +
  6.78 s). The Windows-only `windows_reader_is_bounded_and_quiet_off_screen` (COM + automation object + a point off every
  display, within budget) is among them;
- 350db0bf `cargo-test -p computer --lib -- --ignored windows_hit --nocapture` → **1 passed** (job 20261009T011335-9):
  the centre of the primary display (1920×1080 at that moment, Sky's RDP session) resolved in 403 ms to a `pane` of
  `msedgewebview2` (the theuth desktop window's WebView2 content, before the tree was built) — which motivated the
  Chromium retry and the owner-pid fix;
- 417f70c8 the same ignored test with the ancestor dump → **1 passed** (job 20261009T011759-17): in 166 ms the hit is now
  `image "Overview 0 need you · 0 agents working · …"` and the chain reads
  `image → group → group → group → document → pane(View) … pane("tauri.localhost - Web content") → pane("tauri.localhost",
  Chrome_WidgetWin_1, pid 7920 msedgewebview2) → pane(WRY_WEBVIEW, pid 6808) → window("Tauri Window", pid 6808
  theuth-desktop, empty title) → root("Desktop 1")`; `frontmost()` = `theuth-desktop`, no title (the window has none);
- 298dead0 (owner pid = the top-level window's) `cargo-test -p computer --lib` → **47 passed, 0 failed, 1 ignored**
  (job 20261009T012311-25, 13 s build + 7.13 s);
- **browser round** (desk lock held 03:12:40–03:14:30, Edge launched maximized on `https://github.com/basedatum/theuth`
  through windows-mcp, then the ignored test at 298dead0, job 20261009T021310-89) → **1 passed**, the hit in **80 ms**:
  `element: image "404 “This is not the web page you are looking for”"`, `front: { app: "msedge", window_title: "Page not
  found · GitHub · GitHub - Profile 1 - Microsoft Edge", document_url: "https://github.com/basedatum/theuth" }`;
  `frontmost()` returned the same app/title/URL. That is the full `[screen …]` header for a browser — app, title and page
  URL — from UI Automation. (The repo is private, hence GitHub's 404 page; the URL is the one navigated to.) Edge was
  closed (`process kill msedge`, 8 processes, all launched by this round) and the desk released early.

Evidence: `~/.theuth/briefs/2026-10-09/149-windows/evidence/we1/` — `bb-check-*.json`, `bb-test-lib-*.json`,
`bb-test-ignored-*.{json,log}` (the 417f70c8 log carries the full ancestor chain, the `…-edge-298dead0.log` the browser
hit), `mac-check.log`, `mac-test.log`, `mac-clippy.log`; the desktop screenshot of the Edge round is on build-B at
`C:\Users\user\tools\pkg\we1-edge-hit.png`. The console logs show `·`/`“` as `┬╖`/`ΓÇ£`: the build service's console
is not UTF-8; the strings in the engine are UTF-8.

Later commits (417f70c8, 298dead0) change only lines inside `cfg(target_os = "windows")`, so on the Mac only
`cargo fmt -p computer --check` was re-run for them (clean); check/test/clippy on the Mac compile none of those lines.

### 2.2 R1 (refused lookups → per-display `accessibility: off`), 532369c3

- Mac: `cargo fmt -p computer` clean; `mbx check -p computer` rc 0; `mbx test -p computer` → **53 passed, 0 failed**
  (+ `a_refused_hit_carries_nothing_else`, `suffix_is_off_only_when_every_lookup_was_refused`,
  `refused_lookups_are_counted_and_add_nothing`); `mbx clippy -p computer --all-targets -- -D warnings` rc 0 (03:18:43 →
  03:20:19, warm target).
- build-B: `cargo-check -p computer --tests` → **0 errors, 0 warnings** (job 20261009T021851-95, 7 s warm); `cargo-test -p
  computer --lib` → **50 passed, 0 failed, 1 ignored** (job 20261009T022523-115, 10.6 s build + 4.03 s).
- A live elevated-window check was **not** run: an elevated window needs a UAC prompt that build-B cannot answer
  unattended. The refusal path is exercised only by the unit tests (the rule) and by code reading (every UIA call checked;
  the three HRESULTs and a failed pid read map to `refused`).

## 3. Deviations from the design

1. ~~**`accessibility: off` per display for UIPI (149 §9)** not implemented~~ — **resolved in R1** (532369c3): refused
   lookups are distinguished from "nothing there" (`AxHit.refused`), and a display whose every lookup was refused says
   `accessibility: off`; mixed displays stay `on`. The reading chosen for a budget overrun / a worker that never started is
   also "refused" — the tree was not available for that point, so "only the picture is available" is the honest line.
2. **`app` is an exe stem, not a display name.** Windows has no `localizedName`; `QueryFullProcessImageNameW` is what the
   brief asked for. The version-resource "FileDescription" ("Microsoft Edge") would be friendlier but costs a file read
   per hit; not done.
3. **No workspace `windows` dependency exists** (the brief said "use the workspace `windows` crate"): `meeting` pins
   `=0.54.0` for flexaudio, `desktop` uses `windows-sys 0.61`, xcap brings `windows 0.62.2`. `computer` declares
   `windows = "0.62"` so the graph gains no crate version; `Cargo.lock` gains one edge. No versions bumped.
4. **Owner process = the top-level window's**, not the hit element's (the brief's "CurrentProcessId → exe stem" applied
   to the element would name `msedgewebview2` for every Tauri/WebView2 and Chromium page). Falls back to the element's pid
   when no top-level window resolves.
5. **The Chromium retry** is not in the brief; without it the first hit on a Chromium surface (browsers, Electron,
   WebView2 — including theuth's own desktop window) is a nameless pane. It mirrors the macOS reader's
   `AXManualAccessibility` retry (3 × 120 ms, once per process).

## 4. Open items

- **Windows clippy** (`cargo clippy -p computer --all-targets -- -D warnings` on build-B) cannot run through the build
  service — owed; the Windows build shows 0 rustc warnings.
- **Live UIPI check**: an elevated window under the pointer was not exercised (UAC prompt, unattended box); the refusal
  mapping rests on the documented HRESULTs and the unit tests.
- **Multi-display**: `physical_point` and the per-display logic are built for several monitors, verified on build-B's single
  display only.
- **DPI awareness of `t.exe`**: the engine has no manifest, so xcap's `scale_factor` takes its `GetDeviceCaps` fallback;
  geometry stays `DEVMODE` physical and this reader does not depend on the scale, but a capture under a DPI-unaware process
  on a > 100 % display should be checked once the Windows companion sends real samples (build-B runs at 100 %).
- **Firefox**: the Document-`Value` URL path is from Firefox's documented UIA behaviour, not exercised on build-B (only
  Edge is installed; Edge verified live, see §2).
- **Elevated windows** were not exercised (nothing elevated was on the desktop); the code path is "every call checked →
  `None`".