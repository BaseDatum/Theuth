# 58 step 11 — the island on Windows and Linux, as built, 2026-09-23

Branch `feature/island-xplat`, base `067bfa8`. Scope: 58 §6.2's per-window-system
contract as one Tauri command set with a backend per OS. macOS is the as-built
`NSPanel` module moved verbatim (`git` reports a 100 % rename); its behaviour is
unchanged.

## Shape

```
crates/desktop/src/island/
  mod.rs      commands on every desktop OS; `island_capabilities`; the generic
              Windows/Linux `island_show/hide/place/resign_key`, `set_hidden_from_capture`
  macos.rs    the old island.rs, byte for byte (NSPanel, notch, screens-changed)
  windows.rs  init: WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST on the HWND;
              show: SW_SHOWNOACTIVATE + SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)
  linux.rs    init: set_always_on_top + set_skip_taskbar; show: plain show
```

`lib.rs` registers the six commands on every desktop OS from one
`generate_handler!`; only `tauri_nspanel::init()` stays macOS-gated. The island
window moved from `tauri.macos.conf.json` into the shared `tauri.conf.json`
(same entry); the macOS config still overrides the whole `windows` array, so the
macOS bundle is identical.

## Contract per OS (58 §6.2) → `island_capabilities()`

| | hide_from_share | always_on_top | non_activating | placement |
|---|---|---|---|---|
| macOS | true (`sharingType`) | true (status level) | true | `notch` |
| Windows | true (`WDA_EXCLUDEFROMCAPTURE`, Win10 2004+) | true | true | `top-centre` |
| Linux X11 / Wayland | **false** | true (EWMH `ABOVE` / best effort) | false | `user` |

The island webview reads it once (`lib/tauri.ts` `islandCapabilities`); when
`hide_from_share` is false the eye button becomes a non-interactive eye with the
title "Visible in screen shares: this window system cannot hide the island from
capture". Off Tauri (the `/island?fixture=meeting` browser page) the binding
answers the macOS row so the fixture renders as before.

## Judgement calls

- **Capture hiding on Windows is Tauri's `set_content_protected`** — tao
  implements it as `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`, exactly
  the brief's call, on both windows like macOS. No second code path.
- **No `x11rb`.** GTK's `gtk_window_set_keep_above` / `set_skip_taskbar_hint`
  / `set_skip_pager_hint` send the very `_NET_WM_STATE` client messages the
  brief names (`_NET_WM_STATE_ABOVE`, `_SKIP_TASKBAR`, `_SKIP_PAGER`), and tao's
  `set_always_on_top` / `set_skip_taskbar` call them. `linux.rs` re-asserts
  them after creation; an XID round-trip would be the same request twice.
  `set_content_protected` is not called on Linux (tao returns unsupported).
- **Show/hide off macOS is plain `ShowWindow`/`gtk_widget_hide`**, not the
  alpha-0 trick: the alpha trick exists because WebKit throttles an off-screen
  webview until its RPC client never connects (03 §9); WebView2 and WebKitGTK
  keep a hidden window's scripts and sockets running. If the Windows build
  shows the island's RPC not connecting while hidden, switch `island_hide` to
  `set_ignore_cursor_events(true)` + a layered alpha of 0.
- **Placement** (Windows and Linux share it): `positions[key]` from the
  webview wins (a drag, saved per topology key exactly as on macOS); else
  top-centre of the work area of the monitor remembered for this topology in
  `app_config_dir/island-monitors.json` (`key → monitor name`, beside
  the window-state plugin's file); else the monitor under the cursor, which is
  then remembered; else the primary. Topology key = sorted
  `name:x,y,w,h` per monitor joined by `;`. On Wayland `cursor_position`
  fails and `set_position` is ignored by the compositor — the fallback chain
  answers the primary's top-centre and the compositor places the window.
- **`island_resign_key` is a no-op** off macOS: on Windows the island never
  activates; on Linux the WM owns focus. `non_activating` is honest about it.
- **No `screens-changed` off macOS.** The webview resets `placed` on every
  hide, so the next show re-places; a topology change mid-meeting waits for
  the next show. `WM_DISPLAYCHANGE` would need a window subclass; GDK's
  `monitors-changed` a `gdk` dependency. Add when someone hits it.
- **`Settings › Meetings`** still shows the macOS wording for the capture
  switch; not in this brief's allow-list, works on Windows (WDA), is a no-op
  `Ok` on Linux.

## Observability: `THEUTH_ISLAND_PROBE=1`

No meeting recorder exists off macOS yet, so nothing shows the island. With the
variable set, `island::init` logs the capabilities, every monitor (name,
position, size, work area, scale), the cursor, then calls `island_place` and
`island_show(440, 44)` and logs the result plus the backend's read-back
(Windows: `hwnd`, `ex_style` word and the three flags; Linux: display server,
`is_visible`, `is_always_on_top`, `is_focused`). Expected on the Windows box:
the empty strip top-centre of the cursor's monitor, no taskbar/alt-tab entry,
the foreground app keeps focus, black in a screenshot/Teams share.

## Verified

- macOS: `cargo check -p desktop`, `cargo check --workspace --all-targets`,
  `cargo build -p theuth`; `tsc -b --noEmit` clean for `Island.tsx` and
  `lib/tauri.ts` (other files fail on the worktree's missing `@openuidev/*`
  packages and generated bindings — pre-existing, unrelated).
- Windows: `cargo check -p desktop --target x86_64-pc-windows-msvc` stops at
  `ring` / `aws-lc-sys` build scripts (reqwest via the updater). The island
  module alone was checked for that target in a throwaway crate (`tauri 2`
  without plugins + `windows-sys 0.61` + `raw-window-handle 0.6`): clean.
- Linux: not checkable from the Mac (no target, gtk-sys needs pkg-config);
  the orchestrator's Ubuntu `cargo check -p desktop` covers `linux.rs`.

New deps (Windows only): `raw-window-handle 0.6`, `windows-sys 0.61`
(`Win32_Foundation`, `Win32_UI_WindowsAndMessaging`). Linux: none.
