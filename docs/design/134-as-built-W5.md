# 134 — Companion for Windows — as built, W5 (Tauri shell on Windows + bundling, `feat/134-w5-shell`)

Base `3d931f2c` (main). Rust/Tauri/packaging only (D10–D12, §5.1, §5.5–5.7); Windows-gated, additive, mirroring
`DESK/companion/macos.rs`. The C# app (`crates/desktop/companion-windows/**`) is W1–W4's and is not touched; the
contract relied on: `build.ps1 -Configuration Release` publishes to `target/release/companion-windows/` with
`Theuth.Companion.exe` at the root, the companion holds `Local\io.theuth.companion` while it runs, its rail window
class is `TheuthCompanionRail`, it accepts `--launched-by-shell` / `--background` / `--probe-notify N`, and it
mirrors the Run key into `<data_dir>\companion.json`. `DESK/` = `crates/desktop/src/`.

## Files

| Path | What |
|---|---|
| `DESK/companion/windows.rs` (new) | `init` (`:42`): `mutex_held()` → `set_alive`; if free, `locate()` (`<exe dir>\companion\Theuth.Companion.exe`, else `%THEUTH_COMPANION_APP%\Theuth.Companion.exe`, `:119`) → `launch` (`std::process::Command` + `--launched-by-shell`, `:132`), else one info line; `watch` (`:166`): 2 s `tokio::time::interval`, `set_alive(mutex_held())` on the main thread. `alive()` (`:58`). `terminate()` (`:64`): return if the mutex is free; `FindWindowW("TheuthCompanionRail")` → `GetWindowThreadProcessId` + `PostMessageW(WM_CLOSE)`; poll the mutex ≤ 2 s; then `OpenProcess(PROCESS_TERMINATE)` + `TerminateProcess`, or `taskkill /F /IM Theuth.Companion.exe` when no rail window named a pid (`ponytail:` marked). `mutex_held()` (`:106`): `OpenMutexW(SYNCHRONIZE)` + `CloseHandle`. `set_alive` (`:146`): idempotent swap, emits `companion-alive`, `tray_by_id("main").set_visible(!alive)`. `login_item_status()` (`:183`): `companion.json` `launch_at_login` → `enabled` / `not_registered` |
| `DESK/companion/mod.rs` | three-way switch: `macos` · `windows` (`:12-15`) · `other` for the rest (`:16-19`); `companion_login_item_status` Windows arm (`:51-54`); module doc covers both companions |
| `DESK/companion/other.rs` | doc: Linux only |
| `DESK/lib.rs` | plugin block (`:278-290`): `tauri_nspanel` stays macOS; `tauri_plugin_single_instance::init(\|app, _, _\| show(app))` on Windows **before** `tauri_plugin_deep_link::init()`, which is now `any(macos, windows)`; `setup` calls `deep_links` under the same gate (`:345-346`); `deep_links` (`:528-543`) gate widened and, new, replays `app.deep_link().get_current()` before registering `on_open_url` (see Deep-link facts); `deep_link_route` and its test unchanged (47 tests) |
| `DESK/update.rs` | `install_pending` (`:132-142`): `crate::companion::terminate()` before `update.install` — idempotent after the `RunEvent::Exit` call at `lib.rs:56`; doc explains the Windows reason |
| `crates/desktop/Cargo.toml` | Windows table (`:76-83`): `windows-sys` + `Win32_System_Threading`; `tauri-plugin-single-instance = { version = "2", features = ["deep-link"] }` (resolved 2.4.5; `Cargo.lock` +18 lines, `windows-sys 0.60.2` and `zbus` already present — no new `windows`/`windows-core` minors) |
| `crates/desktop/tauri.windows.conf.json` | `plugins.deep-link.desktop.schemes = ["theuth"]`; `bundle.windows.nsis.installerHooks = "nsis/hooks.nsh"` |
| `crates/desktop/nsis/hooks.nsh` (new) | `NSIS_HOOK_PREINSTALL` / `NSIS_HOOK_PREUNINSTALL`: `nsExec::ExecToLog 'taskkill /F /IM Theuth.Companion.exe'` (result popped, ignored); `NSIS_HOOK_POSTUNINSTALL`: `DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "TheuthCompanion"` unless `$UpdateMode = 1` |
| `scripts/package-windows.ps1` | header lines for 3b and `THEUTH_SKIP_COMPANION_SMOKE`; step `[3b/6]` (`:183-201`): when `crates/desktop/companion-windows/build.ps1` exists → `pwsh -NoProfile -File build.ps1 -Configuration Release`, require `Theuth.Companion.exe`, run `--probe-notify 3` (≤ 60 s, exit 0; skippable), print file count + size; else one skip line; the rendered bundle config (`:211-219`) adds `resources["../../target/release/companion-windows/"] = "companion/"` only when built; done summary names the companion |

Not touched: `crates/desktop/companion-windows/**`, `login_item.rs`, `tauri.conf.json`, `tauri.bundle.conf.json`,
`capabilities/*` (`deep-link:default` already listed), `UI/**`, the CLI zip step.

## §5.6 items

| Item | Met by |
|---|---|
| `companion/windows.rs` replaces `other.rs` on Windows: launch at start, mutex alive detection, `companion-alive`, tray hide/show, terminate | `windows.rs` as above; `mod.rs:12-19` |
| `init`: launch if the mutex is free, `--launched-by-shell` | `windows.rs:42-56`, `:132-144`. `CreateProcess` is std's `Command::spawn`; no `CREATE_NO_WINDOW` needed for a WinExe. The `Child` is dropped (std does not kill on drop). Our launch flips alive at once (the Mac does the same); the poll corrects it within 2 s if the process dies before creating the mutex |
| `watch`: poll the mutex every 2 s | `windows.rs:166-180` (`run_on_main_thread`, as `macos.rs`) |
| `set_alive`: emit + `tray_by_id("main").set_visible(!alive)` | `windows.rs:146-163` (verbatim from `macos.rs`) |
| `terminate`: `WM_CLOSE` to `TheuthCompanionRail`, 2 s, `TerminateProcess` | `windows.rs:64-103` |
| `lib.rs`: single-instance first with deep-link handoff, deep-link plugin on Windows, `deep_links()` gate widened | `lib.rs:281-290`, `:345-346`, `:528` |
| `tauri.windows.conf.json` scheme | done; NSIS writes `HKCU\Software\Classes\theuth` (`installer.nsi:671-677`) and deletes it on uninstall (`:806-812`) |
| `update.rs` terminate before the installer | `update.rs:137`; plus the existing order at `lib.rs:56-58` |
| `companion_login_item_status` Windows arm | `mod.rs:51-54` → `windows.rs:183` |
| `UI/App.tsx` | nothing new (platform-neutral since 130 W1S) |

## §5.7 items

| Item | Met by |
|---|---|
| step `[3b/6]` runs `build.ps1` when present, else a skip line | `package-windows.ps1:183-201`; the test is on `build.ps1` itself (the thing invoked), not the `.slnx` the design names |
| S2 toast smoke on the release publish output | `Theuth.Companion.exe --probe-notify 3`, exit 0 within 60 s (`:191-196`); `THEUTH_SKIP_COMPANION_SMOKE=1` bypasses it |
| `resources["../../target/release/companion-windows/"] = "companion/"` | `:214-215`; a directory key is walked and each file lands at `companion\<relative>` (tauri-utils 2.9.3 `resources.rs:196-205`, `:263-275`) |
| NSIS uninstall hook removes the Run value | `nsis/hooks.nsh` via `installerHooks` |
| CLI zip unchanged | yes |

## `installerHooks` finding (was UNVERIFIED)

Supported by the pinned toolchain. `cargo-tauri` on this Mac is **tauri-cli 2.11.4** (`~/.cargo/.crates.toml`), which
pins `tauri-bundler = "2.9.4"` and `tauri-utils = "2.9.3"` (`tauri-cli-2.11.4/Cargo.toml:265-269`). In those sources:

- `tauri-utils-2.9.3/src/config.rs:909-939`: `NsisConfig.installer_hooks: Option<PathBuf>` (`installerHooks` /
  `installer-hooks`), documenting exactly four macros — `NSIS_HOOK_PREINSTALL`, `NSIS_HOOK_POSTINSTALL`,
  `NSIS_HOOK_PREUNINSTALL`, `NSIS_HOOK_POSTUNINSTALL`.
- `tauri-cli-2.11.4/src/helpers/config.rs:121` passes it to `tauri_bundler::NsisSettings.installer_hooks`.
- `tauri-bundler-2.9.4/src/bundle/windows/nsis/mod.rs:397-402`: `dunce::canonicalize` of the path (the CLI has
  `set_current_dir(tauri_dir)` at `build.rs:158`, so `nsis/hooks.nsh` resolves against `crates/desktop/`) →
  template data `installer_hooks`.
- `installer.nsi:34-35` `!include "{{installer_hooks}}"`; insertion points `:641-643` (Install, before
  `CheckIfAppIsRunning`), `:733-735` (end of Install), `:778-780` (Uninstall, before `CheckIfAppIsRunning`),
  `:886-888` (end of Uninstall). `$UpdateMode` is a global `Var` (`:74`) set from `/UPDATE` in both `.onInit`
  (`:488-490`) and `un.onInit` (`:770-772`), so the POSTUNINSTALL guard is valid.

build-B's `cargo-tauri` version is **not verified** from here; the hooks shipped in tauri-bundler 2.1 (Tauri v2.1,
2024-11), so any v2 CLI since then accepts the key. `nsExec` is a stock NSIS plugin (`Plugins\x86-unicode\nsExec.dll`
in the NSIS zip Tauri downloads) — first exercised by the packaging gate.

## Deep-link facts

- With the `deep-link` feature, `tauri-plugin-single-instance` calls `DeepLink::handle_cli_arguments(argv)` **before**
  the user callback (`single-instance-2.4.5` ≈ `2.4.2/src/lib.rs:72-74`), which emits `deep-link://new-url` →
  `on_open_url` → `deep_link_route` → `navigate`. The callback then `show(app)`s so a plain second launch (no URL)
  also raises the window; on a link it is a redundant show.
- **Cold start by a link** (no instance running; `shell\open\command "theuth-desktop.exe" "%1"`): the deep-link plugin
  parses argv inside its own `setup` (`deep-link-2.4.10/src/lib.rs:73-83`, `:196-228`, requires the scheme in
  `plugins.deep-link.desktop.schemes`) and emits before our listener exists; `get_current()` holds the URL, so
  `deep_links()` now replays it (`lib.rs:540-542`). On macOS `current` is `None` at setup and `RunEvent::Opened`
  still drives the listener — unchanged behaviour. [INFERENCE] the `navigate` event emitted during `setup` can
  precede the webview's `onNavigate` subscription on a cold start; that race already exists for the macOS
  `RunEvent::Opened` path and is left as is (the packaging gate's deep-link step runs against a running app).
- `handle_cli_arguments` only treats a **single** argument as a link; the companion sends `theuth://attention/<id>`
  etc. as the one argument (`Launcher.LaunchUriAsync` → the registered command), which matches.

## Deviations

- `terminate()` falls back to `taskkill /F /IM Theuth.Companion.exe` when no `TheuthCompanionRail` window exists to
  name a pid (the design says `TerminateProcess` only). A Toolhelp32 walk is the upgrade; marked `ponytail:`.
- Step 3b tests for `build.ps1` rather than `Theuth.Companion.slnx` (the design's wording): the script calls
  `build.ps1`, so that is the file whose absence means "skip".
- The S2 smoke is `--probe-notify 3` exit 0 (W3's flag, the design's "`--probe-notify` then asserts `Register()`
  succeeds") rather than a bespoke `Register()` + `Show` harness; a `THEUTH_SKIP_COMPANION_SMOKE=1` escape exists
  because the probe's behaviour in a non-interactive ssh session is W3's to prove.
- `install_pending` calls `companion::terminate()` itself in addition to the existing `RunEvent::Exit` order; the
  design names only "before `install_pending`". Idempotent (returns at once when the mutex is free).
- `companion_login_item_status` on Windows reads only `companion.json` (`enabled` · `not_registered`); it does not
  consult the Run key itself (D12: the companion owns both and mirrors).
- `other.rs` is kept for Linux rather than deleted (the design says `windows.rs` "replaces" it on Windows only).

## Gates run

- Mac (`CARGO_TARGET_DIR=…/theuth-134-w5/target`, through `build-lock`): `cargo fmt --check -p desktop` clean (the
  workspace-wide `cargo fmt --check` reports pre-existing diffs in `crates/agent` and `crates/app`, not this wave's);
  `cargo check -p desktop` ok (41 s, lock resolved `tauri-plugin-single-instance 2.4.5`); `cargo clippy -p desktop
  -- -D warnings` ok (`Checking desktop` confirmed after a `touch`); `cargo test -p desktop --lib deep_link` 1 passed.
- build-B `cargo-check` (LAN build service, slot `w5`, `crates/desktop/Cargo.toml`, Debug): `580dc305` → exit 0 in
  161 s (cold), one warning `companion\mod.rs:24 constant BUNDLE_ID is never used` — fixed by gating `BUNDLE_ID` to
  macOS; the follow-up sha's result is in the final message.
- Not run here: `pwsh` is not installed on this Mac, so `package-windows.ps1` was not parse-checked; the Tauri CLI
  was never run (rule).

## Packaging gate (orchestrator, after W2's tray/rail lands)

1. On build-B, in a detached worktree at the merged sha: `pwsh -NoProfile -File scripts/package-windows.ps1`
   (set `THEUTH_SKIP_COMPANION_SMOKE=1` if `--probe-notify` cannot post from the ssh session). Expect
   `==> [3b/6] companion` → `~452 files, ~170 MiB`, `cargo tauri build` listing `companion\…` resources, and
   `target/release/dist/theuth_<ver>_windows-x86_64-setup.exe` + `…windows-x86_64.zip` (zip = the three exes only).
2. Install silently: `& target\release\dist\theuth_<ver>_windows-x86_64-setup.exe /S`; wait for it to exit. Check
   `Test-Path "$env:LOCALAPPDATA\theuth\companion\Theuth.Companion.exe"`, `onnxruntime.dll` beside `t.exe`, and
   `reg query HKCU\Software\Classes\theuth\shell\open\command` → `"…\theuth-desktop.exe" "%1"`.
3. Launch `%LOCALAPPDATA%\theuth\theuth-desktop.exe` from the RDP session (un-elevated window in front). Expect in
   the shell log: `companion: launched …\companion\Theuth.Companion.exe` then `companion: alive=true; tray hidden`;
   `windows-build-b window_management list` filtered to `Theuth.Companion.exe` + `theuth-desktop.exe`; exactly one
   tray icon; the companion posts a toast (`Register()` ok on the installed layout).
4. Deep link while running: `Start-Process 'theuth://attention/<id>'` (or `app`) → the main window shows on
   `/attention/<id>`; `Get-Process theuth-desktop` still shows **one** process.
5. Kill the companion by process (`Stop-Process -Name Theuth.Companion`) → within ≤ 2 s `companion: alive=false;
   tray shown` and the Tauri tray icon returns; relaunch it by hand → alive again, tray hidden.
6. Quit from the tray → shell log shows no `TerminateProcess` warning; `Get-Process Theuth.Companion` is empty.
7. Launch at login: enable it in the companion's Settings → `reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v TheuthCompanion`
   and `companion.json` `launch_at_login: true`; the main UI's status reads `enabled`.
8. Uninstall: `& "$env:LOCALAPPDATA\theuth\uninstall.exe" /S` → the Run value is gone (`reg query … /v TheuthCompanion`
   fails), `HKCU\Software\Classes\theuth` is gone, `companion\` is gone except files the companion wrote at runtime
   (`settings.json`, `logs\` — the template's non-recursive `RMDir` leaves them; see Open).
9. Update path (optional): with the previous version installed and running, run the new setup.exe by hand → the
   PREINSTALL hook's `taskkill` line appears in the install log and the copy over `companion\` succeeds.

## Open / unverified

- Everything above the Mac gates is type-checked on build-B only; the behaviour (mutex visibility across the
  shell/companion pair, `WM_CLOSE` acceptance by the rail, NSIS hooks, `nsExec`) is first exercised by the packaging
  gate.
- The in-app updater is inert on Windows today: `update::check` requires a `.app` ancestor (`bundle_dir()`), so no
  update is ever parked and `install_pending` returns early; the terminate-before-install order is in place for when
  that changes.
- `companion\settings.json` and `companion\logs\` live inside the install dir (D12 chose `%LOCALAPPDATA%\theuth\companion\`),
  so an uninstall leaves that folder behind; a POSTUNINSTALL `RMDir /r "$INSTDIR\companion"` would delete user
  settings and is not added.
- The companion's `Register()` leaves `HKCU\Software\Classes\AppUserModelId\…` / `CLSID\{…}` entries; not removed by
  the uninstaller (not in the design).
