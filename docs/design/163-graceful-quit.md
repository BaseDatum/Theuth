# 163 as-built — graceful quit (no beach ball, the shutdown shown)

Branch `fix/163-quit`, base `cc757054`. Brief: `~/.theuth/briefs/2026-11-03/163-164/BRIEF-163-QUIT.md`.

## The ask (Sky, 2026-11-03)

> When I do quit theuth in the menu bar app, I get the beach ball on the window I assume while the application is saving it's state before quitting.  that should be handled gracefully and display the shutdown process to the user.

## Mechanism (before)

Every quit entry (tray › Quit, ⌘Q, `theuth://quit`, Settings › Install and restart) called `desktop::request_quit`: emit `quit-requested`, then `update::exit` on `quit_ack` or after 3 s. `app.exit(0)` → `RunEvent::Exit`, which ran on the main thread: `companion::terminate()` (up to 2 s), then `Launch::shutdown()` → `stop()` (up to `EXIT_GRACE` = 12 s while the engine drains its turns), then the bundle swap. The window stayed on screen while the run loop stood still, so macOS showed the beach ball for up to ~14 s and nothing said what was happening.

## As built

### D1 — the main thread never waits (`crates/desktop/src/quit.rs`)
- `quit::request` (all four entries: `lib.rs` menu `"quit"` and deep link, `update::install_update`) sets the `requested` latch, emits `quit-requested { restart }` and arms the `ACK_TIMEOUT` (3 s) fallback thread.
- `quit::start` (`commands::quit_ack`, the fallback, or a failed emit) runs the worker once (`Quit::start`, `started` latch). The worker thread runs in order: `quit-progress {companion}` + `companion::terminate()` (only while `companion::alive()`), `{engine}` + `Daemon::stop(EXIT_GRACE)`, `{engine, forced: true}` when it had to kill, `{closing}`, then `update::exit`. `run` returns right after spawning.
- `launch.rs`: `stop()` became `Daemon::stop(self, grace) -> bool` (`true` = killed); `Launch::take_daemon` hands the daemon to the worker; `Launch::shutdown` keeps the old behaviour for the fallback; `Daemon::tree` gives the cap a killer; `EXIT_GRACE` is `pub`.
- `RunEvent::Exit` → `quit::on_exit`: hide windows (D3), `companion::terminate()`, `Launch::shutdown()`, `update::install_pending`. After the worker the companion is gone (`running()` empty / mutex not held) and the daemon slot is empty, so the first two return at once.
- Off-main-thread safety, checked:
  - macOS `NSRunningApplication` is documented thread-safe, and objc2-app-kit 0.3 does not mark it main-thread-only (`runningApplicationsWithBundleIdentifier` / `terminate` / `forceTerminate` take no `MainThreadMarker`). Both calls send an event and return. The exit check was already `kill(pid, 0)`, not `isTerminated`.
  - Windows `FindWindowW`, `PostMessageW` (posts and never waits on the rail's thread), `OpenMutexW` and `TerminateProcess` have no thread affinity.
  - `AppHandle::exit` / `request_restart` (tauri 2.12.5) only post an exit request to the event loop.
  - Nothing needed `run_on_main_thread`. The doc comments of both `terminate()`s now say who calls them.
- Decision: the daemon is taken out of `Launch` when the worker starts, not at the engine step. That way the cap can kill it whichever step the worker is stuck in. Nothing else uses the daemon once a quit has started (`connection` returns the cached value).

### D2 — bounded
- `CAP = DRAIN (10 s) + EXIT_GRACE (12 s) + 3 s` = 25 s from the worker's start. A cap thread waits on a channel the worker drops after it has exited. On timeout it runs `ProcessTree::kill()` and then `exit`. The kill is `killpg(SIGKILL)` on the pty child's session on Unix (portable-pty `setsid`s it) and the Job Object on Windows.
- `exit` is wrapped once (`AtomicBool`), so the worker and the cap never exit twice.
- Every wait is a named constant (`ACK_TIMEOUT`, `DRAIN`, `CAP`, `EXIT_GRACE`, `EXIT_POLL`). `Plan` takes `grace`, `cap`, the companion step, `emit` and `exit`, so the tests run in milliseconds.

### D3 — exits that bypass the handshake
`on_exit` hides every webview window before any wait. No `prevent_exit`.

### D4 — events
- `quit-requested { restart }`, where `restart` comes from `Pending.restart`.
- `quit-progress { step: "companion" | "engine" | "closing", forced: bool }`. `forced` is always present; the TS type has it optional.
- The `companion` step is emitted only while `companion::alive()`, so Linux (`other.rs` is never alive) and companion-less runs skip it.
- A companion started less than one poll (2 s) before the quit reads as not alive. `on_exit`'s `terminate()` catches it, with the windows already hidden.

### D5 — the overlay (`ui/src/lib/quit-progress.ts`, `ui/src/components/launch/QuitOverlay.tsx` + `.css`)
- **Pure reducer.** Events: `requested` (restart, owned, `at`, the LOCAL engine's slices and `list_sessions` rows), `slices` (every LOCAL sessions-store change), `progress`. `quitSteps` / `quitTitle` turn the state into the rows and the title.
- **Running turns.** Top-level, non-archived sessions with a turn in flight. The slice decides for its session; the list row decides only where no slice exists. A turn is ticked when `running` clears (`TurnFinished`) or when a suspended banner newer than the request appears (`TurnSuspended` from the drain). Everything still open is ticked on `closing`, because the engine has exited and the drain is over.
- **Rows.**
  - "Saving your conversations": the turns, then "Nothing to save" or "The engine keeps running" (`owned` false), with the drain bar while active.
  - "Closing the companion": only after a `companion` step.
  - "Stopping the engine": only for an owned engine. "stopped after the time limit" when forced.
  - "Closing".
- **Component.** `useQuit` (zustand) holds the state.
  - The overlay is fixed, full-window and built from theme tokens only (light and dark). It has no buttons.
  - `role="alertdialog"` with `aria-modal`, focused on mount, `aria-live="polite"` on the step list, and screen-reader status text per row and per turn.
  - A spinning `Loader2Icon` spinner in the header.
  - The drain bar is a CSS `scaleX` animation of `DRAIN_MS` = 10 s, offset to the request time. The UI has no `durability.drain_secs`, so the bar uses the nominal value.
- **`App.tsx`.**
  - `quit(restart)` dispatches `requested` first, then RPC `shutdown` (if owned) and `quit_ack`.
  - The shell's `quit-requested` calls `quit(p.restart)`. `/exit` and the startup screen's Quit call `quit()` too, so every quit shows the overlay. A second `requested` only adds `restart`.
  - `onQuitProgress` and a `scope(LOCAL).sessions` subscription feed the reducer.
  - The app tree sits in a `display: contents` wrapper that turns `inert` while quitting. The overlay mounts in the main window only (`!AUX`).

### D6 — hidden window
`quit::request`: when the main window is not visible it is not shown. The tray tooltip becomes "theuth — shutting down…" and the tray menu becomes one disabled item, "Shutting down…". On macOS/Windows with a companion running, the tray is hidden (130 D9). It comes back within one poll after the companion step, already showing the shutting-down menu.

### D7 — update path
`install_update` → `quit::request`, so it gets the same worker and overlay ("Restarting to update"). `update::exit` still decides between exit and relaunch, and the bundle swap stays in `RunEvent::Exit` (`install_pending`).

### D8 — out of scope, untouched
The engine drain, the Swift/C# companions, `#[cfg(mobile)]` and close-to-hide.

## Tests and proof

- `crates/desktop/src/quit.rs` tests. The fake daemon is a real `sleep 30` on a pty via `Daemon::fake`. It waits for the child's first output byte so the hang-up never races `trap`.
  - `steps_in_order_and_back_at_once`: `run` returns in under 20 ms, then companion → engine → closing → exit.
  - `no_companion_step_without_one`
  - `kills_after_the_grace_and_says_forced`: a SIGHUP-ignoring child; exit comes after the 300 ms grace with `[engine, engine+forced, closing]`, exactly once.
  - `a_second_trigger_is_a_no_op`: the second plan is never built.
  - `the_cap_kills_and_exits`: the worker is stuck in the companion step for 600 ms and the cap is 200 ms. The cap exits between 200 and 600 ms. The worker then finds the daemon dead with no `forced`, well inside its 20 s grace, and does not exit a second time.
  - `payloads`: the wire shapes.
- `crates/desktop/ui/src/lib/quit-progress.test.ts`: steps from events, two running turns ticking (finish and suspend), a pre-quit suspension and subagents/archived not counted, nothing running, reused engine, worker step statuses, forced engine, the restart title (also when it arrives as the second request), nothing happens before a request.
- Screenshots (throwaway, not committed): `/tmp/163-164/quit-proof/png/{requested,saved,stopping,closing,forced,restart}-{1000,390}-{light,dark}.png`. They come from a vite page that mounts `QuitOverlay` over a fake app and drives it through the reducer, captured with headless Chrome 156 over CDP.

**Proven here:**
- The worker's order, that it does not block, the grace kill, idempotence and the cap (Rust tests on macOS).
- The reducer's logic (vitest).
- The overlay's layout in light and dark at 1000 and 390 px (Chrome screenshots).

**Only a live run on Sky's next build shows:**
- That ⌘Q / tray Quit no longer beach-balls.
- That the overlay appears in WKWebView and the steps arrive in order.
- That `on_exit`'s hide takes effect before the main thread blocks. `orderOut` reaching the window server without a run-loop turn is expected but not verified.
- The tray tooltip and menu for a hidden window.
- The companion terminating from the worker thread.
- `request_restart` from the worker on the update path.
- That turns tick from real `TurnFinished` / `TurnSuspended` events.

## Residual risks

- **Windows not compiled or tested here.**
  - The `#[cfg(windows)]` paths are `Daemon::stop` (unchanged logic), `Daemon::tree` (a `ProcessTree` clone that keeps the Job open until the cap thread ends, which is right after the worker exits) and the Windows `Daemon::fake` (`ping -n 30` / `cmd /c exit`). All were reviewed by hand.
  - Linux was not run either; there, `companion` is a no-op and tray tooltips are unsupported by the OS.
- **Tray tooltip can be overwritten.** The webview's attention badge (`useEventSideEffects` → `badge`) can replace the "shutting down…" tooltip if the attention list changes during the quit. Cosmetic.
- **Ticking for a non-active LOCAL engine.** The LOCAL slices see events only while LOCAL is the active engine. With a remote engine on screen, the turns tick together at `closing` instead of one by one.
- **Open dialogs.** An open Radix dialog is portalled outside the `inert` wrapper. The overlay (`z-[100]`) covers it for the pointer, but its focus trap may keep keyboard focus.
- **pid reuse in the cap kill.** The cap's `killpg` targets the daemon's pgid. If the cap fires in the microseconds between the worker reaping the daemon and dropping its channel, the pgid could in theory have been recycled.
- **No 3 s fallback for webview-initiated quits.** `/exit` and the startup screen's Quit do not go through `quit::request`, so they get no fallback timer. They rely on `quit_ack`, as before this change.
