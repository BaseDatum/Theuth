# 121 — As built, W1 CAPTURE: ScreenCaptureKit is the default macOS screenshot path

Branch `feat/121-w1-capture`, cut from d06b08c3. Scope: `crates/computer/**`
only. No dependency, Cargo.toml (root) or Cargo.lock change. Measured on the
same machine as the spike (Apple Silicon, macOS 26.6.2, Screen Recording
granted), one 2560×1440 display at scale 1.

## 1. What changed

| File | Lines | Change |
|---|---|---|
| `crates/computer/src/screen.rs` | 1–12 | Module header: SCK default, per-call `screencapture` fallback, `THEUTH_CAPTURE_BACKEND` documented. |
| | 121–124 | `capture` doc: `tmp_dir` receives the fallback's output. |
| | 177–198 | `enum Backend { Auto, Sck, ScreenCapture }` + pure `fn backend(Option<&OsStr>) -> Backend`. |
| | 200–214 | `grab` reads the env once per call and dispatches: `Sck` → `grab_sck`, `ScreenCapture` → `grab_screencapture`, `Auto` → `grab_auto(.., grab_sck)`. |
| | 215–243 | `grab_auto`: SCK, then the shell-out on any error except a lost grant; `static FELL_BACK: AtomicBool` picks `warn!` (first) vs `debug!` (later). The SCK step is a `fn` parameter for test injection. |
| | 245–249, 294–308 | `grab_screencapture` / `grab_sck` stay `pub` + `#[doc(hidden)]` for the bench; spike wording removed; D3 "never switch to `capture_image()` + `rgba_data()`" in the doc comment and an inline comment at the `capture_sample_buffer` call (L342). |
| | 410–488 | Tests (§4). |
| `crates/computer/examples/capture_bench.rs` | 1–23 | Header rewritten: modes, flags, how to read the output. |
| | 107–108, 193, 310 | Spike wording removed; temp dir `capture-bench`. |
| | 364–388 | Composite row `default` (env unset = production policy) next to `screencapture` / `sck`; `composite-default.png` saved. |
| `crates/computer/build.rs` | 1 | Comment wording only (`ScreenCaptureKit link fixes (121 §6)`); link-search and rpath unchanged. |
| `crates/computer/Cargo.toml` | 28 | Comment on the `screencapturekit` dep. |

Public API unchanged: `capture`, `capture_display_native`, `Shot`,
`CaptureError`, `displays()`, `tmp_dir` kept. `lib.rs` untouched (its header
does not name the capture mechanism).

## 2. Dispatch policy and env override

`THEUTH_CAPTURE_BACKEND`, read on every `grab` (one display), no config setting:

| Value | `Backend` | Behaviour |
|---|---|---|
| unset, `""`, anything else (incl. `SCK`) | `Auto` | `grab_sck`; on error → `grab_screencapture` for the same display |
| `sck` | `Sck` | `grab_sck` only; errors surface (diagnosis) |
| `screencapture` | `ScreenCapture` | shell-out only |

Permission preflight unchanged: `capture` / `capture_display_native` return
`NoScreenRecording` (after `request_screen_recording()`) before either backend.

## 3. Fallback semantics

Per call. Any `grab_sck` error — `SCShareableContent`, display id not found,
filter/config, `capture_sample_buffer`, pixel-buffer checks, older macOS —
falls back, except: if the error is `NoScreenRecording` or
`screen_recording_granted()` is false at that moment, `grab_auto` returns
`NoScreenRecording` without running the shell-out (mirrors
`grab_screencapture` on a non-zero exit).

Exact log lines (target `computer::screen`):

```
WARN  ScreenCaptureKit failed, falling back to screencapture display=<index> error=<SCK error text>   # first fallback in the process
DEBUG ScreenCaptureKit failed, falling back to screencapture display=<index> error=<SCK error text>   # every later one
```

The success path logs nothing.

## 4. Tests

`screen::sck_tests`:
- `backend_from_env_value` — unset / `sck` / `screencapture` / `SCK` / `""` / `garbage`.
- `grab_sck_works_off_main_thread` — SCK from a plain `std::thread`; skips with
  `eprintln!` when Screen Recording is not granted.
- `auto_falls_back_to_screencapture_when_sck_fails` — injects a failing `fn`
  into `grab_auto`, asserts the shell-out produced the display at
  `sck_pixel_size`; skips like the above.
- `auto_reports_lost_grant_instead_of_falling_back` — injected
  `NoScreenRecording` is returned as such (no grant needed; synthetic `Display`).

Frame/composite/context tests unchanged and passing. Gates (all through
`build-lock`, `CARGO_TARGET_DIR=…/theuth-121-w1-target`, `CARGO_BUILD_JOBS=4`):
`cargo fmt -p computer` (no diff), `cargo clippy -p computer --all-targets --
-D warnings` clean (2m 21s), `cargo test -p computer`:

```
test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s
```

## 5. Bench (debug build, `cargo run -p computer --example capture_bench -- --save /tmp/sck-wave`)

Display #1 id=2 2560×1440 @(0,0) scale 1. 20 timed grabs after 3 warm-ups.

| Backend | min | median | p95 (ms) | Pixels | Raw bytes |
|---|---|---|---|---|---|
| screencapture (spawn + PNG decode) | 219.4 | 231.4 | 238.3 | 2560×1440 | 14 745 600 |
| sck (content + capture_sample_buffer + copy) | 49.7 | 50.7 | 54.7 | 2560×1440 | 14 745 600 |
| sck-cg (rejected route, reference) | 51.2 | 52.0 | 55.6 | 2560×1440 | 14 745 600 |

SCK pieces: `SCShareableContent::get()` 25.7 median, on-screen-only getter
12.5, `capture_image` alone 27.5, filter build 0.0, uncached full-`get()` route
67.4, cached-filter route 41.0.

Parity, back to back on a still desktop: dimensions equal (2560×1440); mean
|ΔRGB| per channel **sck vs screencapture 0.001** (≤ 0.1 required); sck-cg vs
screencapture 1.481 (CoreGraphics render, not shipped).

Composite `capture(None, 1568)` (debug build — resize + PNG encode dominate):

| Backend | ms | Size | PNG bytes |
|---|---|---|---|
| screencapture | 1446.9 | 1568×882 | 352 189 |
| sck | 1305.5 | 1568×882 | 352 169 |
| default (env unset) | 1289.5 | 1568×882 | 352 394 |

`default` tracks `sck`, not `screencapture` (−157 ms, ≈ the per-grab delta):
the production policy goes through SCK. No fallback line was emitted during
the run (SCK never failed).

Eyeballed under `/tmp/sck-wave/`: `shot-sck-1.png` vs `shot-screencapture-1.png`
and the three `composite-*.png` — same dimensions; top-40-row crops
(`crop-menubar-*.png`, cut with a CoreGraphics one-off) show the identical menu
bar (Preview menus, status items, clock 9:08 AM) in both; mouse at (1698,439)
inside the display, no cursor drawn in either.

The per-grab SCK numbers match the spike's release run (49.4 median there);
the grab is OS-bound, so the debug build changes only the composite rows.

## 6. Retina status

Retina (scale 2) unverified — only a 2560x1440 scale-1 display attached
(`displays()` reported one display, scale 1).

## 7. Deviations

- No `--release` bench: the cloned target dir had no release artefacts, and
  the per-grab numbers are already within 1.3 ms of the spike's release
  medians. Composite timings above are therefore debug-build and comparable
  only to each other.
- `backend()` is case-sensitive (`SCK` → `Auto`), as the brief's "anything
  else → D1 policy" implies; tested.
- One extra test beyond D7 (`auto_reports_lost_grant_instead_of_falling_back`)
  because the lost-grant branch is the only non-obvious line in `grab_auto`.

## 8. Follow-ups

- Cache `SCDisplay` + `SCContentFilter` per process (invalidate on display
  reconfiguration): this run measured 12.5 ms for the on-screen-only
  `SCShareableContent` getter and 50.7 → 41.0 ms median with a cached filter
  (the spike: 14–24 ms). Not done in this wave (D4).
- `grab_auto` has no success-path log; a `debug!` with the backend used would
  make "which path ran" visible in daemon logs without the bench.
- Binaries linking `computer` still need the Swift rpath (W2).
