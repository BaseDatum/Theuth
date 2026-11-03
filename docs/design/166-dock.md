# 166 as-built — theuth in the Dock and the ⌘Tab switcher

Branch `feat/166-dock`, base `c8295004`. Brief: `~/.theuth/briefs/2026-11-03/166/BRIEF-166.md`.

## The ask (Sky, 2026-11-03 15:35)

> can you also make it where the macos app is no longer a menubar only app. Like I want the application switcher on macos to be able to show me theuth UI.

## Before

- `lib.rs` setup: `app.set_activation_policy(tauri::ActivationPolicy::Accessory)` on macOS → no Dock icon, never in ⌘Tab.
- The main window hides on close (`WindowEvent::CloseRequested` → `prevent_close` + `hide`; `popout-*` close for real). The tray icon `main` toggles it; the first show of a fresh profile (no window-state file) moved it under the tray (`tauri_plugin_positioner`, `Position::TrayCenter`).
- Quit: ⌘Q and tray › Quit are our own menu item `quit` → `quit::request` (163). `theuth://quit` too. A `terminate:` (what Dock › Quit, logout and AppleScript `quit` send) went `applicationWillTerminate:` → `RunEvent::Exit` → `quit::on_exit`: no overlay, main thread blocked while the engine drained (163 "Mechanism (before)" — tao has no `applicationShouldTerminate:`).

## As built

### D1 — activation policy `Regular`
- `lib.rs` setup: `app.set_activation_policy(tauri::ActivationPolicy::Regular)`. Dock icon and ⌘Tab entry whenever theuth runs. The tray icon and the companion's menu bar icon are untouched.
- Module doc of `lib.rs` updated; stale "Accessory" comments in `lib.rs` (app menu, menu bar) and `island/macos.rs` (`set_hides_on_deactivate`) reworded.
- How it applies (read in the registry sources):
  - tauri 2.12.5 `src/app.rs:1304` `App::set_activation_policy` → tauri-runtime-wry 2.12.3 `src/lib.rs:3003-3006` → tao 0.37.2 `src/platform/macos.rs:341-345`, which only stores it in the delegate's aux state.
  - tao `src/platform_impl/macos/app_state.rs:283-292`: `launched` (from `applicationDidFinishLaunching:`) calls `apply_activation_policy` (`:447-459`, `setActivationPolicy:` `Regular`) and then `activateIgnoringOtherApps:` (`activate_ignoring_other_apps` defaults to `true`, `app_delegate.rs:105-106`). `Regular` is also tao's default (`app_delegate.rs:105`); the explicit line stays so nobody reads "no policy call" as an accident.
  - `Info.plist` has no `LSUIElement`, so nothing overrides the policy at launch.

### D2 — Dock click (`dock::reopen`)
- `lib.rs` `run`: `RunEvent::Reopen { .. }` → `dock::reopen` (macOS only). Path: tao `app_delegate.rs:78` registers `applicationShouldHandleReopen:hasVisibleWindows:` → `:205-215` `AppState::reopen` → `app_state.rs:312-315` `Event::Reopen` → tauri-runtime-wry `lib.rs:4337-4341` → tauri `RunEvent::Reopen` (`app.rs:279`).
- `reopen`: main window not visible and no quit under way → `desktop::show` (focus included, now `pub(crate)`). Main visible → nothing; AppKit brings the app forward.
- **Deviation:** `has_visible_windows` is ignored. The island and every `overlay-<n>` panel are ordered in for the life of the process (concealed = alpha 0, `island/macos.rs` `conceal`, `point/macos.rs`), so AppKit reports `true` whenever they exist.
- A visible popout does not stop the Dock click from showing main: D2 names the main window, and a Dock click is an explicit "show me theuth". ⌘Tab (D3) does let a popout stand in for main.

### D3 — ⌘Tab (`dock::init` → `watch_activation`)
- Hook: `NSApplicationDidBecomeActiveNotification` through `NSNotificationCenter` + `block2::RcBlock`, the same pattern as the island's screen-parameters observer (`island/macos.rs` `watch_screens`). There is no Tauri hook: tauri's `RunEvent` has no activation variant, and tao's delegate class (`app_delegate.rs:46-89`) implements no `applicationDidBecomeActive:`.
- Main thread: NSApp posts the notification on the main thread and the block has no queue. `is_visible` / `show` from the main thread are handled in place (tauri-runtime-wry `lib.rs:263-266`, `send_user_message`), the same as the tray click handler today.
- Decision, pure: `show_on_activate(launch_done, quitting, visible kinds)` = `launch_done && !quitting && every visible window is a Panel`. Unit test `activation_shows_main_only_when_nothing_but_panels_is_up`.
- `launch_done`: the first activation after `init` does not count (tao's launch `activateIgnoringOtherApps:`, `app_state.rs:292`) unless it comes more than 5 s after setup (`LAUNCH_ACTIVATION`). Reason for the 5 s: under macOS 14+ cooperative activation a login-item start may have its launch activation refused; then the first activation is a real ⌘Tab and must count.
- `quitting` = `quit::requested` (new accessor on the 163 latch): a ⌘Tab or Dock click during a quit leaves a hidden main window hidden (163 D6).
- Deep links and notification clicks: `theuth://attention|c|new|meetings` already `navigate` → `show`; if their activation runs this check first, `show` runs twice, which is idempotent (one `swap` on the first-show flag). `theuth://quit` is opened with `activates = false` by the companion (`AppDelegate.quitTheuth`) and is caught by `quitting` anyway. The UI sends no notifications with click handlers today; a notification click activates theuth and, with main hidden, shows it — wanted.

#### Window inventory (every label created in `crates/desktop` and the configs)

| Label | Created by | Kind | Treatment |
|---|---|---|---|
| `main` | `tauri.conf.json` / `tauri.macos.conf.json` (`visible: false`) | `Main` | Visible → ⌘Tab shows nothing extra; hidden → shown by ⌘Tab / Dock click. The HUD (`ui/src/components/hud`) lives inside this window — it is not a window of its own. |
| `island` | `tauri.conf.json` / `tauri.macos.conf.json` (`skipTaskbar`, `alwaysOnTop`, `focus: false`) → non-activating `NSPanel` (`island/macos.rs` `STYLE_MASK`) | `Panel` | Ignored: always ordered in, a click never activates the app. The meetings widget. |
| `overlay-<n>` | `point::overlay_ensure_on_main` (`point/mod.rs`), one per display → non-activating panel (`point/macos.rs`) | `Panel` | Ignored: always ordered in (alpha 0 when concealed), click-through or non-activating. Point-and-say, summoned by the voice key (a global shortcut, which does not activate the app). |
| `popout-*` | `commands::open_window` | `Popout` | Visible → ⌘Tab brings it forward, main stays hidden. |
| any other label | none today | `Other` | Counts as a normal window (never pops main) — a new always-up panel then costs a missed ⌘Tab show, not a surprise window. |
| `main` (iOS) | `lib.rs` `mobile` | — | iOS only; `dock.rs` is `cfg(target_os = "macos")`. |

The companion's surfaces (Swift: menu bar icon, Attention / agents panels, palette, its island) are another process with `.accessory` policy; they never activate theuth. The links they open activate theuth and are handled above.

### D4 — close, Dock › Quit, logout
- Close (red button / ⌘W) still hides main (`on_window_event`, unchanged); theuth stays in the Dock and ⌘Tab.
- ⌘Q is our own menu item (`app_menu`, `"quit"` → `quit::request`), so it never sent `terminate:` — the brief's "like ⌘Q" assumed otherwise.
- **Deviation (the brief assumed a route that did not exist):** there is no `RunEvent::ExitRequested` handling in `quit.rs`, and a `terminate:` never produces one. tao implements `applicationWillTerminate:` only (`app_delegate.rs:62`, `:130-133`) → `AppState::exit` (`app_state.rs:271-273`) → `Event::LoopDestroyed` → tauri-runtime-wry `lib.rs:4131-4132` → `RunEvent::Exit` → `quit::on_exit`. So Dock › Quit did not reach the graceful quit. With a Dock icon this becomes a main quit entry, so it was fixed:
  - `dock::add_should_terminate` adds `applicationShouldTerminate:` (`Q@:@`) to the class of `NSApp.delegate` (tao's `TaoAppDelegateParent`) with `class_addMethod` at setup. AppKit asks `respondsToSelector:` on each `terminate:`, so adding it after launch works. If a future tao implements it, `class_addMethod` returns `NO`, a warning is logged, and tao's version stays.
  - `terminate_reply(kAEQuitReason)`: the current Apple event carries `kAEQuitReason` ('why?': logout, restart, shutdown and their dialog variants) → `TerminateNow`, the 163 D3 exit path as before. Reason: `TerminateCancel` would abort the user's logout, and `TerminateLater` would park the session end in a modal run loop while our worker exits through `NSApp.stop` (`app_state.rs:402-409`), which never replies. No reason (Dock › Quit, AppleScript `quit`, Activity Monitor › Quit, the companion's `terminate()`) → `TerminateCancel` + `quit::request` → overlay, drain off the main thread, 25 s cap, `app.exit` → `stop:` (`event_loop.rs:234-240`), not another `terminate:`. tauri 2.12.5 / tao 0.37.2 never call `terminate:` themselves (searched), so the hook cannot loop.
  - A second `terminate:` during a quit gets `TerminateCancel` again (`request` is latched). The companion's 30 s fallback `terminate()` now lands here too: it returns "sent", the shell's own 25 s cap has already exited, and `forceTerminate()` only follows a refused send.
  - The reason code is logged (`applicationShouldTerminate reason=… now=…`) so the live check can see which path a quit took.
  - Tests: `a_session_end_is_never_cancelled` (every reason → Now, none → Cancel) and `should_terminate_answers_through_objc` (the real callback installed on a probe class, called through `objc_msgSend`, its registered encoding checked with `AnyClass::verify_sel` — a wrong encoding fails it, checked by mutation).

### D5 — first-show placement: centred
- `show` on the first show of a fresh profile calls `win.center()` instead of `move_window(Position::TrayCenter)`, for every entry (tray, Dock, ⌘Tab, deep link). Reason: a Dock app's first window belongs in the middle of the screen — under the menu bar icon reads wrong after a Dock click at the bottom — and `TrayCenter` only works once the tray reported its rect, which never happens when the companion owns the menu bar icon (it logged an error and left the window where AppKit put it). Window-state restores the user's placement afterwards, as before.
- `TrayCenter` was the only use of `tauri-plugin-positioner`: the plugin, its `on_tray_event` call, the workspace and crate dependencies, the `positioner:default` capability and its `Cargo.lock` entries are gone. The UI never used its JS API.

### D6 — out of scope (follow-ups)
- Dock badge with the attention count (`set_badge_label` exists in tao `platform/macos.rs:406`; the tray title already carries the count).
- Dock right-click menu (New chat, Start meeting) — `applicationDockMenu:` on the same delegate.
- A Settings toggle back to menu-bar-only (`set_activation_policy` at runtime, tao `set_activation_policy_at_runtime`).
- ~~Showing the window when the user starts theuth from the Dock / Finder while it is not running~~ — done in R1 below.
- Windows / Linux: unchanged. Windows' `main` has no `skipTaskbar` (`tauri.conf.json`; `tauri.windows.conf.json` overrides only `trayIcon` and `bundle`), so it is already in the taskbar and Alt+Tab; only the island has `skipTaskbar: true`.
- The companion's `quitTheuth` doc comment still says `terminate()` "lands straight in the shell's exit path"; since 166 it lands in the graceful quit. Not touched here (companion gate).

## Files

- `crates/desktop/src/dock.rs` (new): activation observer, `reopen`, `applicationShouldTerminate:`, the two pure decisions, 4 tests.
- `crates/desktop/src/lib.rs`: `Regular`, `mod dock`, `RunEvent::Reopen` arm, `dock::init` in setup, `show` `pub(crate)` and centred, positioner removed, doc/comments. The Edit submenu block is untouched (165).
- `crates/desktop/src/quit.rs`: `requested()` accessor; module doc lists Dock › Quit.
- `crates/desktop/src/island/macos.rs`: one stale comment.
- `Cargo.toml`, `crates/desktop/Cargo.toml`, `Cargo.lock`, `crates/desktop/capabilities/default.json`: `tauri-plugin-positioner` removed.

## Gates

1. `cargo fmt -p desktop` → clean (`--check` exit 0).
2. `BUILD_LABEL=166 ~/.theuth/bin/build-lock mbx clippy -p desktop --all-targets --no-deps -- -D warnings` → exit 0; the only warning line is the pre-existing future-incompat note for the dependency `block v0.1.6`. Log `/tmp/166/clippy.log`.
3. `BUILD_LABEL=166 ~/.theuth/bin/build-lock mbx test -p desktop --lib --no-run` → exit 0; `mbx test -p desktop --lib` → 58 passed, 0 failed (4 new in `dock::tests`). Logs `/tmp/166/test-build.log`, `/tmp/166/test.log`.
4. Companion not touched.

Not run: the app itself (Sky's theuth is running on this Mac). Windows / Linux not compiled here; the new code is `cfg(target_os = "macos")` apart from the `show` visibility and the positioner removal.

## Sky's live check (after landing)

1. Start theuth. A theuth icon is in the Dock; the menu bar icon (tray or companion) is as before. (Before R1 the main window stayed hidden here; since R1 a user launch shows it — item 12.)
2. ⌘Tab lists theuth. With main hidden, ⌘Tab to theuth → the main window appears, focused, centred on a fresh profile (else where it was).
3. Close main with the red button and with ⌘W → it hides; theuth stays in the Dock and in ⌘Tab. ⌘Tab away and back → main shows again.
4. With main hidden, click the Dock icon → main shows. With main visible, click the Dock icon → nothing besides coming forward.
5. Open a popout (a note in its own window), close main, ⌘Tab away and back → the popout comes forward, main stays hidden. Click the Dock icon → main shows.
6. Start a meeting so the island is up, close main, use the island (click its buttons, type in its request box) and press the voice key (point overlay) → main does not pop.
7. Click a companion card / island pill (a `theuth://` link) with main hidden → main shows on that route, once.
8. ⌘H theuth with main hidden and a popout open, ⌘Tab back → the popout returns, main stays hidden (AppKit's unhide/activate order is the unverified part here).
9. Dock › Quit → the shutdown overlay / tray "shutting down" as for ⌘Q, no beach ball; the log has `applicationShouldTerminate … now=false`. During the quit, ⌘Tab or a Dock click does not show a hidden main window.
10. Log out (or restart) with theuth running → the logout is not interrupted by theuth; the log has `applicationShouldTerminate reason=Some("…") now=true`.
11. Companion › Quit theuth → graceful as in 163, main stays hidden.
12. (R1) Quit theuth, start it from the Dock, from Spotlight, and with the companion's "Open theuth" → the main window shows each time, once, focused. Turn on Settings › General › Open at login, log out and in → theuth starts with no window. `~/Library/LaunchAgents/theuth.plist` lists `--autostart` after the binary.
13. (R1) With a login item from before R1: the first start after the update logs `login item rewritten with --autostart`; the first login after the update may still show the window once, later logins do not.

## R1 (2026-11-03, after main 246d19aa)

### Merge
- `git merge --no-ff main` (165 PASTE-LINKS): clean, no conflicts. 165's `PASTE_PLAIN` Edit-menu item, its `on_menu` arm, `paste_plain()` and `paste_plain_accelerator_parses` (uses `tauri_plugin_global_shortcut::Shortcut`, unaffected by the positioner removal) are kept and pass.

### A user launch shows the window; a login launch stays hidden
- Why: as a Dock app, a launch from the Dock, Finder, Spotlight or the companion's "Open theuth" (`AppDelegate.swift`, `activates = true` when the main app is not running) showed no window — the brief's kept hidden start. This replaces the D3 "do not show at launch" decision for user launches; the login launch keeps it.
- `lib.rs`: `tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![AUTOSTART_ARG]))`, `AUTOSTART_ARG = "--autostart"`. The plugin writes it into the LaunchAgent's `ProgramArguments` (macOS), and into the `Run` value (Windows) and the `.desktop` entry (Linux) on the next enable there. Only macOS reads it; Windows and Linux behave as before (hidden start).
- `dock::launch`, last step of setup on macOS: `rewrite_login_item`, then `show` when `show_at_launch(argv[1..])`, i.e. `--autostart` is not among the arguments (exact match; `-psn_…` and anything else count as a user launch). Tests `only_the_login_item_launch_stays_hidden`.
- No double show: the launch activation that follows is still skipped by `LAUNCH_ACTIVATION` (5 s), and if it were not, main is visible by then so `show_on_activate` says no; `show` itself is idempotent (the first-show centring is a `swap`).
- Old login items: a LaunchAgent plist written before R1 lacks the arg. At macOS startup, when the plugin reports autostart enabled (`is_enabled` = the plist exists in LaunchAgent mode, auto-launch 0.6 `macos.rs:209-213`) and `~/Library/LaunchAgents/<product>.plist` (`get_file`, `macos.rs:240-251`) does not contain `<string>--autostart</string>`, `enable()` rewrites it (a plain file write, `enable_launch_agent`, `macos.rs:115-130`) and logs `login item rewritten with --autostart`; a failure logs a warning. Test `old_login_items_are_told_apart`.
  - "Once" = only while the arg is missing, so a later start does not rewrite it again.
  - Only when the running binary is inside an `.app` (`…/.app/Contents/MacOS/`): `enable()` writes the *current* executable's path, and a dev build must not repoint Sky's login item at `target/`.
  - Accepted: the first login after the update ran from the old plist (no arg) and shows the window once; the rewrite happens during that start, so the next login is hidden.
- A cold start by a `theuth://` link (companion card while theuth is not running) is a user launch: it shows the window and `navigate` routes it — same window, one show.

### Files (R1)
- `crates/desktop/src/lib.rs`: `AUTOSTART_ARG`, the autostart plugin's args, `dock::launch` at the end of setup.
- `crates/desktop/src/dock.rs`: `launch`, `show_at_launch`, `rewrite_login_item`, `plist_has_autostart_arg`, 2 tests; module doc.

### Gates (R1, after the merge)
1. `cargo fmt -p desktop` → clean (`--check` exit 0).
2. `BUILD_LABEL=166 ~/.theuth/bin/build-lock mbx clippy -p desktop --all-targets --no-deps -- -D warnings` → exit 0; only the pre-existing `block v0.1.6` future-incompat note. Log `/tmp/166/r1-clippy.log`.
3. `BUILD_LABEL=166 ~/.theuth/bin/build-lock mbx test -p desktop --lib --no-run` → exit 0; `mbx test -p desktop --lib` → 61 passed, 0 failed (6 in `dock::tests`, 165's `paste_plain_accelerator_parses` included). Logs `/tmp/166/r1-test-build.log`, `/tmp/166/r1-test.log`.
