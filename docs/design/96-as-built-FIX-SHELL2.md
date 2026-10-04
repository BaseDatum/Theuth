# 96 as built — FIX SHELL2 (annotate mode: hold the left button to draw — pen state, mouse-catching overlays, `point_configure`)

Spec: `docs/design/96-point-and-say.md` §0.4–0.6, §4. Brief: `/tmp/mc-2026-10-04/omp-SHELL2.md`. Previous as-built: `96-as-built-SHELL.md`.

## Branch + base sha

`feat/96b-shell`, base `main` @ `ead30467` (the brief names `52a70f15`; main had moved one site-only commit, `fix(site): bring the pyramid back …`, which touches nothing in this seam). Worktree `/Users/Sky.Moore/Develop/theuth-wt-96b-shell`.

## Commits (oldest first)

- `2832b206` feat(desktop): annotate mode — hold the left button to draw: pen state in samples, mouse-catching overlays, point_configure (96 SHELL2)
- (this note) docs(design): 96 as-built FIX SHELL2

## What changed

- `Cargo.toml:308-311`: `objc2-app-kit` workspace features gain `"NSEvent"` (for `NSEvent::pressedMouseButtons`); comment updated. No new crate: `Cargo.lock` unchanged (gate 5).
- `crates/desktop/src/point/mod.rs`:
  - `Annotate { Always, Mouse }` `:47` with `parse(&str) -> Result<Self, String>` `:53` (`"always"` | `"mouse"`, anything else → `Err`) and `pen(left_button_down) -> bool` `:64` (`Always` → `true`; `Mouse` → the button).
  - `Point.annotate: Mutex<Annotate>` `:78`, default `Always` `:89`.
  - `#[tauri::command] point_configure(point, annotate: String) -> Result<(), String>` `:176`.
  - `SamplesPayload.pen: bool` `:207`; `Sample.pen: bool` `:217` (so `point:stop.samples[i].pen` is on the wire with that exact name).
  - `Effect::Start` `:284-289`: snapshots the mode, reveals the overlays with `catch_mouse = annotate == Mouse`, hands the mode to the sampler (`start_sampler(annotate)` `:389`).
  - `sample(…, annotate)` `:487`: per tick `pen = annotate.pen(backend::left_button_down())` `:511`, recorded in the path and sent in the tick's `point:samples`. Position is sampled whether the pen is up or down.
  - Overlay builder gains `.accept_first_mouse(true)` `:604`; `overlays_set_revealed(app, on, catch_mouse)` `:635`; every conceal path (`Driver::conceal` `:406` for `Stop`/`Discard`/`Conceal`, vanished-display conceal `:625`) passes `false, false`.
- `crates/desktop/src/point/macos.rs`: `panelize` adds `set_becomes_key_only_if_needed(true)` `:35`; `set_revealed(win, on, catch_mouse)` `:45` sets `ignore_mouse_events(!(on && catch_mouse))` *before* the alpha change, so concealing restores click-through first; `left_button_down()` `:58` = `NSEvent::pressedMouseButtons() & 1 != 0`.
- `crates/desktop/src/point/other.rs`: `set_revealed(win, on, catch_mouse)` = `set_ignore_cursor_events(!(on && catch_mouse))` then show/hide; `left_button_down()` = `false`.
- `crates/desktop/src/lib.rs:276`: `point::point_configure` in `generate_handler!` (one line; see Deviations).
- `point/machine.rs`: untouched. Esc handling untouched.

Panel mask, for the record: `STYLE_MASK = 1 << 7` (`NSWindowStyleMaskNonactivatingPanel` only, `macos.rs:14`), unchanged; `panelize` sets it before `set_level(101)`. Plus `becomesKeyOnlyIfNeeded = true`, the island's rule, since `tauri-nspanel`'s `RawNSPanel` answers `canBecomeKeyWindow = YES` (`raw_nspanel.rs:48`) — without it a caught click could make the (non-activating) panel key.

## Tests written

`point::tests`: `annotate_parses_always_and_mouse_only` (`"always"`, `"mouse"` parse; `"sometimes"` and `"Mouse"` rejected), `pen_follows_the_button_only_in_mouse_mode` (a synthetic button sequence `[f,f,t,t,f,t]`: `Always` → all `true`; `Mouse` → identical to the buttons, i.e. consecutive one-point batches flip `pen` exactly where the button did), `samples_payload_shape` updated for `pen` in both `point:stop.samples` and `point:samples`.

## Gates

Environment: `export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:/opt/homebrew/bin:$PATH`.

1. `cargo test -p desktop point` → `test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.72s` (the previous 23 + `annotate_parses_always_and_mouse_only` + `pen_follows_the_button_only_in_mouse_mode`; 3 `models::` tests match the filter as before).
2. `cargo clippy -p desktop --all-targets --no-deps -- -D warnings` → `Finished `dev` profile [unoptimized + debuginfo] target(s) in 44.36s` (no warnings; `Checking desktop v2.6.0` confirmed in the run after `touch src/point/mod.rs`).
3. `cargo check -p desktop` → `Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 50s`.
4. `cargo fmt --all -- --check` → exit 1; 293 `Diff in` lines, 0 under `crates/desktop/` (`… | grep "^Diff in" | grep -c "crates/desktop/"` → `0`) — pre-existing drift in files this brief did not touch. Touched files were formatted with `rustfmt --edition 2024`.
5. `git diff main..HEAD -- Cargo.lock | grep -E '^[+-](name|version)'` → prints nothing (`Cargo.lock` is byte-identical to main; `bitflags` was already an enabled dependency of `objc2-app-kit` in the lock, so the `NSEvent` feature changed no lock line).

## Deviations

- **Batches are never split, because a tick is one point** (brief: "if the state changed within a tick, split into two batches"). The sampler reads the cursor once per tick and emits `pts: [one point]`, so one tick has exactly one pen state; the invariant "a batch shares one pen state" holds structurally and the test covers the per-tick mapping rather than a grouping routine that would have no caller.
- **The mode is snapshotted at `Start`** (brief does not say): `point_configure` mid-gesture applies from the next press. Avoids an overlay that becomes click-catching while the user is already dragging in an app, and keeps the reveal/conceal pair symmetric.
- **Conceal always restores click-through**, in `always` mode too (a redundant `ignore = true`, which `set_revealed(false)` already did before this change). One code path instead of a remembered flag.
- **`becomesKeyOnlyIfNeeded(true)` added** (not in the brief): see the panel-mask paragraph above; it is what the island does (`island/macos.rs:49`) and the brief requires that keyboard focus never moves to the overlay.
- **`lib.rs` touched for one line** (outside the "You own" list, not in "must not touch"): a `#[tauri::command]` is unreachable without its `generate_handler!` entry; SHELL did the same at `:271-276`.
- **`std::sync::Mutex` kept** for `Point.annotate` (repo rule prefers `parking_lot`): `parking_lot` is not a dependency of `crates/desktop`, and `crates/desktop/Cargo.toml` is features-only in this brief. Same decision as SHELL.
- **Off macOS `pen` is always `false` in `mouse` mode** (brief: "`false` on other OSes until a reader exists"): the ring shows, nothing draws, nothing is recorded as pen-down.
- **Spec §4.2 `Sample { t_ms, x, y }` and §4.4 `point:samples { display_index, pts }`** both gain `pen: bool` (brief wins over the doc).
- **Base sha**: `ead30467`, not the brief's `52a70f15` (main advanced by one site commit before the worktree was cut).

## Needs from siblings

- **PANEL** (`crates/desktop/ui`): call `point_configure(annotate)` with `"always"` | `"mouse"` at boot and on the setting change (it returns `Err(String)` for anything else); `point:samples` now carries `pen: bool` (draw a muted ring, no stroke, while `pen == false`); `point:stop.samples[i]` carries `pen: bool` — forward unchanged to `screen_context`.
- **DAEMON2** (`crates/computer`/`crates/rpc`): `Sample` gains `pen: bool`; ignore `pen == false` points for gestures (the field name is exactly `pen`).

## Not built / Not verified

- Live `NSEvent::pressedMouseButtons()` sampling off the main thread at 60 Hz (the class method takes no `MainThreadMarker` in objc2-app-kit 0.3.2, so it compiles for a tokio thread; behaviour not exercised — the app was not launched: another theuth.app runs on this Mac).
- Click interception: that a press on a revealed overlay in `mouse` mode never reaches the app underneath (a GitHub link must not navigate), that `accept_first_mouse(true)` delivers the first click to the non-key panel, and that neither activates theuth nor moves keyboard focus (`NonactivatingPanel` + `becomesKeyOnlyIfNeeded`).
- Esc while the overlay catches the mouse (the Carbon hot key is unaffected by mouse routing; not exercised).
- Order of `set_ignore_mouse_events(true)` before `set_alpha_value(0.0)` observed only in code.
- Windows/Linux `set_ignore_cursor_events(false)` on a transparent always-on-top window actually receiving the press.
- No `bindings/` were generated: no ts-rs types changed in this seam (payloads are mirrored by hand in `lib/tauri.ts`, PANEL's).
