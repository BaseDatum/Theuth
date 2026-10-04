# 96 as built — SHELL (Tauri shell: voice key, gesture machine, sampler, overlays, events)

Spec: `docs/design/96-point-and-say.md` §0, §3, §4, §10 (P0, spikes 2/3/5), §12. Brief: `/tmp/mc-2026-10-04/omp-SHELL.md`.

## Branch + base sha

`feat/96-shell`, base `main` @ `5bc9ccf8`. Worktree `/Users/Sky.Moore/Develop/theuth-wt-96-shell`.

## Commits (oldest first)

- `a7ba7693` feat(desktop): voice key shell — Ctrl+S hold/double-tap machine, 60 Hz pointer sampler, per-display trail overlays, point:* events (96 §4)
- (this note) docs(design): 96 as-built SHELL

## What changed

- `crates/desktop/src/point/machine.rs` (new, 474 lines): §4.1 constants `:6-18` (`HOLD_MS 200`, `DOUBLE_TAP_MS 300`, `MIN_NOTE_MS 800`, `SAMPLE_HZ 60`, `DONE_MS 6000`, `HINT`); `State` `:23` (`#[serde(tag = "state", rename_all = "snake_case")]`), `Event` `:34`, `Effect` `:48`; pure `step(s, e, now, refused)` `:69`; 17 unit tests.
- `crates/desktop/src/point/mod.rs` (new, 661 lines): managed `Point { tx, state, refused, chord }`; `init` `:52` spawns the driver on `tauri::async_runtime` and listens to `screens-changed` `:64`; commands `voice_key_bind` `:83`, `point_cancel` `:120`, `point_done` `:126`, `point_state` `:132`, `point_refuse` `:138`, `overlay_ensure` `:144` (async); payload structs `:153-192` (`point:start {at_ms, locked, color_hint}`, `point:lock {}`, `point:samples {display_index, pts}`, `point:stop {samples[{t_ms,x,y}], duration_ms, reason}`, `point:cancel {reason, hint}`, `point:refused {reason}`); driver `drive` `:206` + effect executor `Driver::run`; Esc lifetime and timers in `after` `:298` / `arm_tick` `:309` / `register_esc` `:318` / `unregister_esc` `:334`; sampler `sample` `:439` with the unit mapping `locate` `:402` (`[INFERENCE; spike 2]` comment) and `MAX_SAMPLES = 60 × 120` `:41`; overlays `overlay_ensure_on_main` `:516`, `overlays_set_revealed` `:574`, `on_main` `:501` (main-thread hop awaited over a oneshot); 3 unit tests.
- `crates/desktop/src/point/macos.rs` (new): `panelize` `:19` — `to_panel()`, `NonactivatingPanel` mask, level 101, `CanJoinAllSpaces | FullScreenAuxiliary | Stationary | IgnoresCycle`, `hides_on_deactivate(false)`, show once, alpha 0 + ignore mouse; `set_revealed` `:38` — alpha 1/0, ignore mouse always, `order_front_regardless` on reveal.
- `crates/desktop/src/point/other.rs` (new): Windows/Linux — `panelize` no-op, `set_revealed` = `show()`/`hide()`.
- `crates/desktop/src/lib.rs`: `mod point` `:19`; window-state `.with_filter(|label| !label.starts_with(point::OVERLAY_PREFIX))` `:246`; six commands in `generate_handler!` `:271-276`; `point::init(app.handle())?` in setup `:304`; `paint_page_background` skips `overlay-*` `:443` (transparent window — the main window's page colour must not be painted on it).
- `crates/desktop/capabilities/default.json:5`: `"windows": ["main", "island", "popout-*", "overlay-*"]`.
- `island/macos.rs`, `island/mod.rs`: untouched (the ten panel lines are duplicated in `point/macos.rs`, as the brief prefers).

## Tests written

`point::machine::tests`: `idle_press_starts_and_arms`, `refused_press_answers_refuse_not_start`, `auto_repeat_press_is_ignored`, `armed_release_before_hold_is_tap1`, `armed_release_after_hold_sends`, `armed_tick_moves_to_holding_only_after_hold`, `holding_release_sends_or_discards_short`, `second_press_inside_window_locks`, `late_second_press_closes_the_tap_and_starts_anew`, `tap1_tick_after_window_discards_with_hint`, `locked_press_sends_or_discards_short`, `esc_in_every_active_state`, `cancel_in_every_active_state`, `panel_done_only_matters_in_sending_and_replying`, `replying_press_starts_a_new_question`, `full_hold_then_double_tap_sequence`, `state_serialises_tagged_snake_case`.

`point::tests`: `chords_parse_with_the_plugin_parser` (`"Control+S"`, `"Control+Shift+S"`, `"Alt+Space"`, `"Escape"` parse; `"Control+"` does not), `locate_maps_physical_cursor_to_display_local_and_global_logical` (two synthetic monitors, scale 2 and 1; edges exclusive; off-screen → `None`), `samples_payload_shape` (exact JSON of `point:stop` and `point:samples`).

## Gates

Environment: `export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:/opt/homebrew/bin:$PATH`.

1. `cargo test -p desktop point` → `test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.03s` (20 `point::` tests + 3 `models::` tests matched by the filter).
2. `cargo clippy -p desktop --all-targets --no-deps -- -D warnings` → `Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.80s` (no warnings; re-run after `touch src/lib.rs` to make sure the crate was linted).
3. `cargo check -p desktop` → `Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.87s`.
4. `cargo fmt --all -- --check` → exit 1; 48 files with pre-existing drift, 0 under `crates/desktop/` (`… | grep "^Diff in" | grep -c "crates/desktop/"` → `0`). Touched files were formatted with `rustfmt --edition 2024`.

## Deviations

- **`step` takes `refused: Option<&str>`** (spec §4.1 signature is `step(s, e, now)`): the brief allowed either the driver or a reducer input; the input keeps the refuse path pure and tested (`refused_press_answers_refuse_not_start`). `Effect::Refuse { reason: String }` is kept as the spec shape.
- **`Effect::Discard { reason, hint }`** (spec: `Discard { hint }`): `point:cancel` needs `reason` (`"esc"` | `"short"` | `"double-tap-missed"` | `"cancel"`); the reducer is the only place that knows it.
- **`Sending` + `Esc`** emits `Discard { reason: "esc", hint: None }`, which the executor turns into `point:cancel` to the island and overlays (the table's "`Cancel` to the panel"; no `Effect::Cancel` exists in the spec enum).
- **`Armed` + `Released` at ≥ `HOLD_MS` also applies the `MIN_NOTE_MS` check** (spec sends unconditionally). Reachable only when the hold tick is late (a 200–800 ms note would otherwise go out from `Armed` but be discarded from `Holding`); same rule in both states.
- **`Tap1` + `Pressed` after `DOUBLE_TAP_MS`** (spec leaves the cell undefined): `Discard { "double-tap-missed", HINT }` then `Start` as a fresh `Armed { t0: now }` — a late tick must not swallow a press.
- **Esc register/unregister follows state transitions, not effects**: registered in the `Start` effect (guarded by `is_registered`), unregistered in `Driver::after` on every entry into `Idle` or `Replying` from another state. Covers `Sending` + `Cancel` and `Sending` + `PanelDone`, which have no effect in the table.
- **`Tick { now }` carries the time it was armed for; the reducer judges by the clock it is stepped with** (the driver's `Instant` ms). Stale ticks are harmless: every `Tick` rule re-checks the elapsed time.
- **Overlay URL is `/overlay?display=<n>`** (spec §4.5), not the brief's `index.html#/overlay?…`: `tauri.conf.json` gives the island no URL (the UI picks `/island` by window label) and pop-outs use path routes (`commands.rs:37`), so hash routing would be a second convention.
- **Window-state exclusion is `with_filter`, not the denylist**: `tauri-plugin-window-state` 2.4.1's denylist is exact-match (`lib.rs:416`), so `"overlay-*"` would match nothing.
- **`screens-changed` refits overlays only once at least one exists**; before the first gesture nothing is built (the first `Start` builds against the current displays). Avoids three idle webviews for a user who never presses the key.
- **Monitor set is read once per gesture** by the sampler (`ponytail:` comment at `mod.rs:440`); a display attached mid-hold gets its overlay on the next press.
- **`std::sync::Mutex` kept** although the repo rule prefers `parking_lot`: `parking_lot` is not a dependency of `crates/desktop`, and adding it would edit `Cargo.lock`, which the brief reserves for DAEMON.
- **`set_content_protected(true)` applied to each new overlay** (the brief's fallback): `island::set_hidden` stores no eye value; the island webview re-applies the stored setting to every window on its next toggle.
- **Off macOS, concealed = `hide()`** (WebView2/WebKitGTK keep hidden webviews running — the island's generic backend's reasoning); macOS uses alpha 0.
- **No change to the global-shortcut plugin builder**: `on_shortcut` and `unregister` exist in 2.3.2 (`~/.cargo/registry/src/*/tauri-plugin-global-shortcut-2.3.2/src/lib.rs:143,182`); `Shortcut` is `global_hotkey::hotkey::HotKey` with `FromStr`.

## Needs from siblings

- **UI**: call `voice_key_bind(chord)` after boot and on rebind (`""` unregisters); listen to the six `point:*` events with the field names above; route `/overlay?display=<n>` renders the canvas; the island calls `point_refuse("meeting")` / `point_refuse(null)`; `point_state()` returns `{ "state": "idle" | "armed" | "holding" | "tap1" | "locked" | "sending" | "replying", t0?, released? }`.
- **DAEMON**: nothing; `point:stop.samples` is the `Sample { t_ms, x, y }` list in logical global points §6.2 expects.

## Not built / Not verified

- Live Carbon `Pressed`/`Released` on a held Ctrl+S (auto-repeat, missed releases) — spike 3; the reducer ignores a repeat `Pressed` regardless.
- Dynamic rebind to `Control+Shift+S` and back; a chord held by another app returning `Err` and keeping the old binding (the code path registers the new chord before unregistering the old, so a failure leaves the old one in place; not exercised live).
- Overlay appearance, level 101 above menus, click-through, Spaces/full-screen behaviour, alpha-0 concealment, `order_front_regardless` on reveal.
- Mixed-DPI unit mapping in `locate` (`[INFERENCE; spike 2]`): assumes `Monitor::position()` is physical and the logical origin is position over scale.
- Capture exclusion of the overlays (`set_content_protected(true)`) and that `Stop` conceals before `point:stop` reaches the panel in practice (the await ordering is in the code; spike 5 checks pixels).
- `WebviewUrl::App("/overlay?display=<n>")` resolving to the UI's overlay route in a production bundle (pop-outs prove the pattern).
- The `screens-changed` refit (vanished display concealed, new display built) on a real display change.
- The app was not launched (another theuth.app instance is running on this Mac); no `bindings/` were generated (no ts-rs types changed in this seam).
