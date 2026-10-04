# 96 as built — DAEMON: `computer` plugin screen context

Branch `feat/96-daemon`, base `main` @ `5bc9ccf8`. Worktree `theuth-wt-96-daemon`. 2026-10-04.

## Commits (oldest first)

- `ab4b26c2` feat(computer): screen_context contract — types, actor message, RPC stub (96) — the contract UI's worktree is cut from.
- `61fb148e` feat(computer): pointer grammar — loops, strokes, dwells with synthetic-path tests (96)
- `c1f18d4a` feat(computer): accessibility reader ax.rs on objc2-application-services (96)
- `47995927` feat(computer): ax_probe example for spike 4 (96)
- `5c5e0345` feat(computer): screen_context compose — native capture, stamped trail, crops, [screen …] lines, config, prompt block (96)
- `f8065de6` fix(computer): clippy nits in ax, context, pointer tests (96)
- `d7496ceb` fix(computer): export since_ms as a TS number like every other u64 (96) — `ScreenContextReq.since_ms` is `number`, not `bigint`, in the generated TS (the `#[ts(type = "number")]` convention `tasks` uses for `u64`).
- (this note's commit) docs(design): 96 as-built DAEMON

## What changed

- `crates/computer/src/pointer.rs` (new): `Sample` (serde + schemars + ts-rs behind `ts`), `Pt`, `Bbox`, `Gesture`, the seven threshold consts of §6.2, `gestures()` (`:90-104`), `anchor()` (`:108-118`), `loops()` (`:170-205`, return-within-24-pt-of-a-≥300-ms-older-sample or segment crossing, bbox ≥ 20 × 20, IoU > 0.5 dedupe, walk resumes after the closing sample), `strokes()` (`:217-250`, greedy chord extension outside loops, chord ≥ 80, deviation ≤ 15, breaks when the chord shrinks by more than the deviation), `dwells()` (`:254-283`, maximal 12-pt runs ≥ 500 ms, 12-pt dedupe), 12 unit tests.
- `crates/computer/src/ax.rs` (new): `AxElement`, `AxFront`, `granted()`, `truncate()`, `suffix()`, `role_word()`; macOS module with `element_at()` (`AXUIElementCreateSystemWide` → `AXUIElementCopyElementAtPosition` → `AXRole`, `AXTitle`/`AXDescription`, `AXValue`, `AXURL`) and `frontmost()` (`NSWorkspace.sharedWorkspace.frontmostApplication` → `AXUIElementCreateApplication(pid)` → `AXFocusedWindow` → `AXTitle`, `AXDocument` else `AXURL`); CFString / CFURL / CFNumber / CFBoolean values stringified, every `AXError` checked, nulls mapped to `None`. Non-macOS: `granted()` false, readers `None`.
- `crates/computer/src/context.rs` (new): `compose(req, cfg, data_dir)` (`:52-143`): enumerate → visited displays in first-visit order plus the End display (primary fallback) → `gestures()` → gestures assigned to their anchor's display → AX (`frontmost()` once, `element_at()` on the loop centroid + a 3 × 3 grid in its bbox / stroke midpoint / dwell point / end point, deduped by (role, label, url), ≤ `max_elements`, label and value cut to `value_chars`) → per display `capture_display_native` → polyline stamped in physical pixels (6 pt × scale, `req.color` or `#2f5d4a`, alpha 0.85, discs at ≤ 1 px steps = round caps/joins) and a 14 pt ring of width 2 pt at the End → overview PNG ≤ `max_edge` (Triangle) → crops (Loop bbox, Stroke from/to bbox, Dwell 48 × 48; padded 24 pt, clamped, dropped above 60 percent of the display, ≤ `max_crops`, downscaled only above `crop_edge`) → each PNG re-encoded at three quarters of its edge until ≤ 5 MB → written as `<data_dir>/attachments/<sha256[..16]>.png` → `[screen i/n …]` lines per §6.6. `PROMPT` = §6.7 verbatim. `tracing::info!` with `capture_ms`, `draw_ms`, `encode_ms`, `ax_ms`. 9 unit tests.
- `crates/computer/src/screen.rs`: `enumerate()` now `pub(crate)` (`:20`); new `capture_display_native(&Display, &Monitor, tmp)` (`:155-168`) — the existing `grab` without the downscale, same permission check and request as `capture()`. `capture()` untouched.
- `crates/computer/src/lib.rs`: `ScreenContextReq`, `ScreenContextReply` (serde + schemars + `ts_rs::TS` behind `ts`, like `PermissionStatus`), `ScreenContext` actor message, `Message<ScreenContext> for Computer` running `compose` in `spawn_blocking`; `ComputerConfig.screen_context: ScreenContextConfig { max_edge 1568, crop_edge 1024, max_crops 3, accessibility auto|off, max_elements 6, value_chars 80 }` with serde defaults; `Computer.data_dir` (from `rt.data_dir`); the prompt block appended to `tools::PROMPT` in the one `RegisterPrompt`.
- `crates/computer/examples/ax_probe.rs` (new): spike 4 — prints `granted`, pointer, `frontmost()`, `element_at()`; `--watch` polls every second.
- `crates/computer/Cargo.toml`: `sha2`; macOS: `objc2-application-services`, `objc2-core-foundation` (+ `CFURL`, `CFNumber`), `objc2-app-kit` (+ `NSWorkspace`, `NSRunningApplication`, `libc`), `objc2-foundation`.
- Root `Cargo.toml`: `objc2-application-services = 0.3` (`# 96`), `default-features = false`, features `std, HIServices, AXUIElement, AXError, libc`. `Cargo.lock`: exactly one new package, `objc2-application-services 0.3.2`.
- `crates/rpc/src/api.rs`: trait method `screen_context(req) -> RpcResult<ScreenContextReply>` (`with_extensions`); impl refuses `peer.remote()` ("the screen is on the harness host"), maps a missing actor (`computer.enabled: false`) to `NOT_RUNNING` "disabled", otherwise `send`s `ScreenContext(req)`.
- `crates/rpc/src/export.rs`: `computer::ScreenContextReq`, `computer::ScreenContextReply`, `computer::pointer::Sample` registered for `export_bindings`.

## AX crate choice (spike 4)

`objc2-application-services 0.3.2` (madsmtm/objc2, the family `objc2-core-graphics 0.3.2` and `objc2-app-kit 0.3.2` already in the lock come from). It exposes `AXUIElement::new_system_wide`, `copy_element_at_position`, `copy_attribute_value`, `new_application` and `AXIsProcessTrusted`; its dependencies (`objc2-core-foundation 0.3.2`, `objc2-core-graphics 0.3.2`, `libc`, `bitflags`) were all in the lock, so the delta is one package. `accessibility-sys 0.2.0` / `accessibility` (eiz) would also have been one package but are a separate, lightly maintained lineage over `core-foundation 0.9`; the `objc2` crates are maintained and versioned together. `AXIsProcessTrusted` itself stays the existing `input::accessibility_granted()` probe. `ax_probe` run on this Mac (Accessibility granted to the terminal): `granted: true`, frontmost `theuth` / `io.theuth.mac`, element under the pointer `group` — the reader works; Chrome/Safari `AXURL`/`AXDocument` are not verified (no pointer was moved over a browser on Sky's Mac).

## Tests written

`pointer::tests`: `circle_is_one_loop`, `underline_is_a_stroke`, `jittery_hold_is_a_dwell`, `short_dash_is_not_a_stroke`, `open_arc_is_not_a_loop`, `no_movement_is_end_only`, `circle_then_hold_orders_loop_dwell_end`, `two_samples_are_end_only`, `empty_is_nothing`, `two_lobes_are_two_loops`, `stroke_ending_in_a_dwell_reports_both`, `scribble_is_end_only`.
`ax::tests`: `truncate_cuts_on_char_boundaries`, `suffix_and_roles`.
`context::tests`: `line_matches_the_grammar` (the §6.6 example line byte for byte), `line_without_accessibility_keeps_coordinates_local`, `colour_parsing`, `crop_rects_pad_clamp_and_skip`, `physical_mapping_uses_the_real_ratio`, `stamping_paints_the_trail_and_ring_only`, `oversized_png_is_re_encoded_smaller`, `visited_is_first_visit_order_plus_end`, `save_png_uses_put_attachment_naming`.

## Live smoke (not a gate)

A throwaway example (not committed) called `compose()` on this Mac with a synthetic 60-pt circle at (700,400) followed by a 700 ms jittery hold at (1000,600), `accessibility: true`, default config, debug build:

```
[screen 1/1 "theuth" · pointer: circled (640,340)–(760,460): group; underlined (758,384)→(1003,600): group; rested on (1000,600) for 704 ms: group; ended on group · accessibility: on]
```

plus four files under `attachments/` (overview 334 KB at 1568 wide, three crops 2–6 KB). The overview shows the green circle, the straight run to the hold and the end ring; the loop crop is the circle plus 24 pt. The "underlined" is the synthetic jump from the circle's last sample to the hold — a real pointer draws it too, so it is reported.

## Gates

Environment: `PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:/opt/homebrew/bin:$PATH`, rustc 1.98.1.

1. `cargo test -p computer` → `test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.30s` (12 pointer, 2 ax, 9 context, 13 pre-existing).
2. `cargo test -p rpc --features ts export_bindings` → `test export::export_bindings ... ok` / `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 4.15s`; `bindings/methods.json` line 242 is `"screen_context",`; `bindings/` holds `ScreenContextReq.ts` (`samples: Array<Sample>, color: string, since_ms: number, accessibility: boolean`), `ScreenContextReply.ts` (`lines: Array<string>, files: Array<string>`), `Sample.ts` (`t_ms: number, x: number, y: number`). Not committed.
3. `cargo clippy -p computer -p rpc --all-targets --no-deps -- -D warnings` → `Finished `dev` profile [unoptimized + debuginfo] target(s) in 31.61s` (no warnings).
4. `cargo check -p theuth -p tui -p nodes` → `Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 01s` (cold), re-run after the last commit `Finished … in 5.60s`.
5. `cargo fmt --all -- --check` → 293 `Diff in` blocks, 0 of them under `crates/computer/` or `crates/rpc/` (pre-existing drift in `tui`, `theuth`, `tasks`, `talk`, …; the touched files were formatted with `rustfmt --edition 2024`).
6. `git diff main..HEAD -- Cargo.lock | grep -E '^[+-](name|version)'` → `+name = "objc2-application-services"` / `+version = "0.3.2"` — the one intended crate.

Release timing of the live smoke (`cargo build --release`, one 1x display, AX on, default config): 426 ms cold, 217 ms warm from `compose()` entry to return including capture, stamp, two encodes, three crops, AX and the writes — inside §8's ≤ 600 ms target. Debug build: ≈ 2 s.

## Deviations

- **"No movement at all → only End" vs "a 700 ms hold with ±3 pt jitter → Dwell"** (brief, step 2): both are in the brief's test list and a literal §6.2 dwell rule would report the first as a dwell too. Decision: a path whose samples are all the identical point reports `End` alone (nothing was drawn; the End says where); any displacement, even jitter, makes a ≥ 500 ms rest a `Dwell`. `dwells()` carries the rule; both tests pass.
- **Loop walk resumes after the closing sample for the return check too**: the earlier sample `i` is searched from the resume point, so a second lobe closes against its own start, not the first lobe's (the brief's and the spec's figure eight test both pass as `two_lobes_are_two_loops`).
- **Stroke extension stops when the chord shrinks** by more than `STROKE_MAX_DEV_PT` (an out-and-back along one line would otherwise collapse to a zero chord); the spec's rule says only "every point within 15 pt of the chord".
- **`capture_display_native(&Display, &Monitor, tmp)`** takes the monitor and the temp dir (the brief's signature had only `&Display`; off macOS the grab needs the `xcap::Monitor`, on macOS a temp path). Temp files go to `<data_dir>/computer/tmp`, the layout `tools::shot_dir`'s doc already names for durable roots.
- **Dwell crop is 48 × 48 pt** (brief) rather than the spec's 240 × 160; a crop whose gesture covers more than 60 percent of the display is dropped (spec §6.5; the brief is silent, the overview shows it anyway).
- **`imageproc` not adopted, `cargo tree` not run for it**: imageproc 0.25 depends on nalgebra, rayon, ab_glyph and friends — far more than 10 new packages, known from its manifest rather than measured in this tree. Discs stamped at ≤ 1 px steps along each segment give round caps and joins; the downscale softens the edge.
- **No `screen_context.enabled` sub-flag**: §6.8 has none; `computer.enabled: false` is the off switch and the RPC answers `NOT_RUNNING` "disabled" by mapping the missing actor, since a disabled plugin spawns no actor at all.
- **AX off the main thread**: `NSWorkspace.frontmostApplication` and the `AXUIElement*` calls run on the `spawn_blocking` thread; `objc2-app-kit` marks neither as main-thread-only and the probe ran fine from a plain thread. If a target app's AX server stalls, the whole `compose` waits — no per-call timeout was added.
- **Sample timing**: `Sample.t_ms` is `u32`; `since_ms` is passed through unused by the daemon (the shell/UI own the hold duration for the `[voice note, N s]` line).
- **Prompt registration**: appended to the existing `RegisterPrompt` as `tools::PROMPT + "\n\n" + context::PROMPT` — present whenever `computer.enabled`, as §6.7 says.
- **Non-macOS**: `ax::granted()` is `false` there even though `input::accessibility_granted()` answers `true` (enigo needs no TCC off macOS); the line ends `accessibility: off`, as §0.14 says.

## Needs from siblings

- UI: `api.screen_context(req)` mirror in `crates/desktop/ui/src/rpc/api.ts` from the generated `ScreenContextReq.ts` / `ScreenContextReply.ts` / `Sample.ts`; `Sample` fields are `t_ms`, `x`, `y` (snake_case, as the doc writes them); `files` are absolute paths to pass straight to `send_input`/`steer`.
- SHELL: samples in logical global points with the primary display's top-left as origin (the space `AXUIElementCopyElementAtPosition` and xcap's monitor rects share); `t_ms` from the press.
- Nothing needed in `crates/agent`, `voice`, `tui`.

## Not built / Not verified

- Live hotkey → RPC → desk flow (integration; the orchestrator's P1 check).
- `AXURL` on a Chrome/Safari link and `AXDocument` on their windows (spike 4's browser half): the reader reads those attributes but no browser was under the pointer during the probe.
- Mixed-DPI two-display capture: `Scale` uses the real image/logical ratio per display, unit-tested; not exercised on two displays here.
- `screen_context` over the wire (`peer.remote()` refusal, "disabled" mapping): compiled, not called through a live socket.
- Latency on a Retina or multi-display setup (the smoke ran on one 1x display; a 2x display quadruples the pixels the stamp and the overview downscale touch).
