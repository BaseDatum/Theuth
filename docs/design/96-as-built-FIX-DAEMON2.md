# 96 as built — FIX DAEMON2: app/window from the hit element, Chromium trees, pen-aware grammar

Branch `feat/96b-daemon`, base `main` @ `ead30467` (contains design 96 and `96-as-built-DAEMON.md`). Worktree `theuth-wt-96b-daemon`. 2026-10-04.

## Commits (oldest first)

- `7aab72cb` feat(computer): pen-aware pointer grammar — Sample.pen, gestures per pen-down run (96 DAEMON2) — also the accessibility reader rewrite (hit-derived app/window, `AXManualAccessibility` retry, window-level hits dropped, theuth skipped in the header) and the pen-aware drawing.
- `700d2caf` feat(computer): ax_probe reports which apps needed AXManualAccessibility (96 DAEMON2)
- (this note's commit) docs(design): 96 as-built FIX DAEMON2

## What changed

- `crates/computer/src/ax.rs` (rewritten macOS half):
  - `AxFront` gains `pid: i32` (`:31`); `AxHit { element: Option<AxElement>, front: Option<AxFront> }` (`:47`) — one hit-test's element (absent when the hit was the window) plus the app/window owning it.
  - `Reader` (`:57`), one per `compose()`, remembers the pids that already had `AXManualAccessibility` set. `Reader::hit_at(global)` (`:262`): `AXUIElementCopyElementAtPosition` → `AXRole` → `AXUIElementGetPid`; when the hit is window-level (`window_level`, `:185`: role `AXWindow`, or `AXGroup`/`AXWebArea` with an empty `AXChildren`) and the pid is new to this reader, `AXUIElementSetAttributeValue(app, "AXManualAccessibility", kCFBooleanTrue)` on `AXUIElementCreateApplication(pid)`, a `tracing::debug!` line with the pid and the `AXError`, then up to 3 retries of the hit-test with 120 ms sleeps while the hit is still window-level. `AXEnhancedUserInterface` is never touched.
  - `front_of` (`:224`): `NSRunningApplication::runningApplicationWithProcessIdentifier(pid)` → `localizedName`, `bundleIdentifier`; the window via `window_of` (`:210`: `AXWindow`, else `AXParent` walked ≤ 32 levels to role `AXWindow`) → `AXTitle`, `AXDocument` else `AXURL`.
  - `element_of`: role word, `AXTitle` else `AXDescription`, `AXValue`, `AXURL`; a role `AXWindow` yields `element: None`.
  - `is_self(&AxFront)` (`:65`): this pid, this process's bundle id (`own_bundle_id()`, `:303`, `NSRunningApplication::currentApplication`), or a bundle id starting `io.theuth.` (the desktop app — see Deviations).
  - `frontmost()` (`:312`) kept as the last resort, now filling `pid`. Non-macOS: `Reader::hit_at` → `None`, `own_bundle_id` → `None`.
- `crates/computer/src/pointer.rs`:
  - `Sample.pen: bool` with `#[serde(default = "pen_default")]` = `true` (`:22-27`); exported to TS as `pen: boolean`.
  - `gestures()` (`:100`): `End` = the last pen-down sample, else the last sample; the path is split on pen-up samples and `loops`, `strokes`, `dwells` run per run; `loops(run, &mut out)` (`:188`) and `dwells(run, &mut out)` (`:276`) push into shared vectors so the IoU > 0.5 loop dedupe and the 12-pt dwell dedupe apply across runs as before; `loops` returns the covered spans of the run for `strokes`.
- `crates/computer/src/context.rs`:
  - `compose()` (`:68`): gestures first; `end` = `anchor(End)`; `end_display` from it; `ax::Reader` per call; `elements_for(g, cfg, &mut reader) -> (Vec<AxElement>, Option<AxFront>)` (`:452`) collects the elements under the anchors (window-level hits add nothing) and the first anchor's app/window; `header(&fronts)` (`:58`) picks the header: the last gesture's (End's) front, else the latest other front, skipping theuth while another app resolved; theuth itself only when every resolved hit was theuth; `ax::frontmost()` only when no hit resolved an app. `frontmost()` is no longer called up front.
  - Drawing (`:134-142`): pen-up samples map to `None`, so the polyline breaks at a pen-up; the ring is at `End.at`. `visited()` (`:202`) counts pen-down samples only (plus the End display).
- `crates/computer/examples/ax_probe.rs`: prints the `AxHit` (element + owning app/window) through a `Reader` and the pids that needed `AXManualAccessibility`.
- `crates/computer/Cargo.toml`: `objc2-core-foundation` feature `CFArray` added (for `AXChildren`); no new package, `Cargo.lock` unchanged.

## Tests written

`pointer::tests`: `two_circles_split_by_a_pen_up_are_two_loops` (the pen-up travel between them is not a stroke), `pen_up_gap_between_two_points_is_not_a_stroke`, `all_pen_up_is_end_only_at_the_last_sample`, `pen_field_defaults_to_down` (serde default and explicit `false`). The 12 earlier tests are unchanged.
`ax::tests`: `self_is_this_process_or_the_desktop_app`.
`context::tests`: `header_prefers_the_end_hit_and_skips_theuth`; `visited_is_first_visit_order_plus_end` extended with a pen-up crossing that does not visit a display.

## Probe (step 6)

`/Applications` holds Google Chrome, Microsoft Edge and Slack. Google Chrome could not be opened with a window: a sibling's headless Chrome (`--headless=new … --user-data-dir=/tmp/mc-2026-10-04/chrome-prof`) owned the bundle's running-app slot, so `open -a "Google Chrome" <url>` was swallowed by it (AppleScript: 0 windows). Microsoft Edge (Chromium, `com.microsoft.edgemac`) was used instead, on `https://github.com/rust-lang/rust`, the pointer moved with `cliclick`:

```
hit: AxHit { element: Some(AxElement { role: "link", label: Some("Insights"), value: None,
  url: Some("https://github.com/rust-lang/rust/pulse") }),
  front: Some(AxFront { pid: 59650, app: "Microsoft Edge", bundle_id: Some("com.microsoft.edgemac"),
  window_title: Some("GitHub - rust-lang/rust: Empowering everyone to build reliable and efficient software. · GitHub - Microsoft Edge"),
  document_url: Some("https://github.com/rust-lang/rust") }) }
manual accessibility set for: Reader { manual: {} }
```

Other points resolved `text` (with `AXValue`), `button "Platform"`, `link "Synchronize Unicode license text from unicode.org" → https://github.com/rust-lang/rust/commit/6d2a…`. Edge's tree was already live: no hit was window-level, so `AXManualAccessibility` was never set (`manual: {}`). While theuth's own window was frontmost over Edge during the scan, hits resolved to theuth's `group`/`text`/`button` elements with `app: "theuth"` — i.e. the app now follows the element under the pointer, not activation. The `AXManualAccessibility` branch itself (Sky's Chrome case) compiled and is exercised only by the window-level condition; see Not verified.

## Gates

Environment: `PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:/opt/homebrew/bin:$PATH`, rustc 1.98.1.

## Gates (gates 1–4 run by the worker, logs `/tmp/mc-2026-10-04/DAEMON2-gates*.log`; gates 5–6 run by the orchestrator in this worktree at 22:22 after the worker stalled on the write-up)

1. `cargo test -p computer` — `test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.34s`
2. `cargo test -p rpc --features ts export_bindings` — `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 5.27s`; `bindings/Sample.ts` line 23: `pen: boolean, };`
3. `cargo clippy -p computer -p rpc --all-targets --no-deps -- -D warnings` — `Finished dev profile [unoptimized + debuginfo] target(s) in 5m 40s`, `EXIT=0`
4. `cargo check -p theuth -p tui -p nodes` — `Finished dev profile [unoptimized + debuginfo] target(s) in 1m 36s`
5. `rustfmt --edition 2024 --check` on `ax.rs`, `context.rs`, `pointer.rs`, `examples/ax_probe.rs` — exit 0 (no diff)
6. `git diff ead30467..HEAD -- Cargo.lock | grep -E '^[+-](name|version)'` — prints nothing (only the `CFArray` feature was added to `objc2-core-foundation` in `crates/computer/Cargo.toml`)

## Deviations

- **Header fallback order** (brief step 1: "END-point element's app/window; fallback: the first anchor that resolved; last resort `frontmost()`"): implemented as End's front, else the *latest* other gesture's front (gesture order is Loop, Stroke, Dwell, End — the last non-End gesture is the dwell or stroke nearest the end in precedence, and a dwell sits near where the pointer stopped). Same fallbacks otherwise. `header()` is unit-tested.
- **"theuth itself"**: the daemon is a bare executable in a different process from the desktop app, so comparing with this process's pid/bundle id alone would never match the shell's windows. `is_self` also matches bundle ids starting `io.theuth.` (`tauri.conf.json` identifier `io.theuth.mac`). When every resolved hit is theuth (the pointer really ended over a theuth window) theuth is reported.
- **Manual-accessibility trigger**: taken on the "or simply whenever the first hit returns a window-level element" branch of step 2 — no bundle-id/executable heuristic. Setting the attribute on a native app returns an `AXError` that is logged and ignored. To bound the cost, each pid is toggled and retried once per `compose()` (`Reader.manual`): a native app whose title bar was circled costs at most 3 × 120 ms once, not per anchor.
- **Window-level** also covers `AXGroup`/`AXWebArea` with no `AXChildren` (the brief's parenthesis); such a hit still contributes an element (`group`) when the retries do not improve it — only `AXWindow` is dropped from the element list, as step 3 says.
- **`visited()` ignores pen-up samples** (brief step 5 speaks only of drawing): a display crossed with the pen up has nothing drawn on it, so capturing it would send an unmarked screenshot. The End display is still always included.
- **Test "two circles separated by a pen-up → two Loops"** written with two well-separated circles: two identical circles dedupe by IoU across runs "as today", which the brief also asks for, so the identical-circle variant cannot be both.
- **`Sample.pen` in TS is required** (`pen: boolean`); the serde default is for old JSON payloads only. `bindings/` is regenerated, not committed.
- **`AXDocument` comes from the hit element's window** (brief). For Edge the window itself carried `AXDocument`; Safari/Chrome were not under the pointer. If a browser exposes the URL only on its `AXWebArea`, the header will lack the URL — the link elements still carry `AXURL`.
- `ax_probe` no longer calls `element_at` (removed); it prints the `AxHit` and the reader's `manual` set.

## Needs from siblings

- SHELL2: fill `Sample.pen` (`true` while the left button is held in pen mode; always `true` in the hold-to-draw mode). Samples stay logical global; a pen-up sample still carries the pointer position (used only for `End` when nothing was drawn).
- PANEL / CARD: the regenerated `Sample.ts` has `pen: boolean`; nothing else on the wire changed (`ScreenContextReq`, `ScreenContextReply`, method name unchanged).

## Not built / Not verified

- The `AXManualAccessibility` retry path against a Chromium app whose tree is off (Sky's live Chrome case): Edge's tree was already on; Google Chrome had no openable window on this Mac during the run (a sibling's headless instance). The code path is the window-level branch of `Reader::hit_at`; it compiled under clippy `-D warnings`. The `tracing::debug!` line is the evidence to look for at integration.
- `compose()` end to end with the new reader (capture + stamp + AX) was not re-run live here (no `theuth.app` launch allowed; the first wave's smoke covered the pipeline; the AX and grammar changes are unit-tested).
- `AXParent` walk to the window: Edge answered `AXWindow` directly; the walk is untested live.
- Non-macOS build of `ax.rs` (`Reader::hit_at` stub, `own_bundle_id` stub): not compiled here.
