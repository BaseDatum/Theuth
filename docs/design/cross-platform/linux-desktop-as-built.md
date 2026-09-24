# 58 — the Linux desktop shell reaches its own engine, as built 2026-09-24

Branch `fix/linux-desktop-local-engine`. Bug: `theuth-desktop` on Ubuntu arm64
opened on the relay pairing screen. `Launch::connection` had already spawned the
daemon; the webview's `container()` knew `'linux'` but `hasLocal()` only accepted
`'macos'`, so `useNeedsPairing()` was true and the local row was missing.

## What changed

| Was | Now |
|---|---|
| `Container = 'macos' \| 'linux' \| 'ios' \| 'web'`; WebView2 fell through to `'macos'` | `+ 'windows'` (`Windows NT` in the UA, checked after iOS, before Linux); `isDesktop()` = macos \| linux \| windows; `deviceName()` → `Windows` |
| `hasLocal()` = `container() === 'macos'` | `isDesktop()` — every desktop shell has the daemon beside it (`Launch::connection` runs on all three) |
| `LOCAL_NAME = 'This Mac'` | `This Mac` / `This PC` / `This computer` by `container()`; `Info.name` still replaces it once connected |
| `useHasMeetings()` hid Meetings for the Linux shell's own daemon | Gone: the meeting plugin runs on every OS (step 10); the sidebar entry and the settings tab always show |
| `useCanRecord()` = local && `isMacDesktop()` | local && `isDesktop()` && `meeting_status().supported.kind !== 'None'` (58 §3.3: the daemon's own capability, probed per call) |
| `useMeetingHotkeys` / startup `setHiddenFromCapture` on macOS only | on `isDesktop()`: the global-shortcut plugin and the `set_hidden_from_capture` command exist on every desktop OS (the latter no-ops where `island_capabilities().hide_from_share` is false, step 11) |
| `startUpdateListener()` on macOS | unchanged: `update.rs` swaps a `.app` bundle (`bundle_dir`) |
| `isMacDesktop()` (drag region, traffic-light inset, `x-apple.systempreferences` deep links) | unchanged in meaning; Windows now correctly falls outside it (native title bar, `decorations: true`) |

## WebKitGTK without a GPU

A VM without GPU passthrough prints `libEGL warning: failed to get driver name`,
`MESA: error: ZINK: failed to choose pdev`, `DRI2: failed to create screen`, and
WebKitGTK's DMA-BUF renderer may paint a blank window. The standard workaround is
`WEBKIT_DISABLE_DMABUF_RENDERER=1`. `desktop_lib::run` (`lib.rs`,
`desktop::webkit_no_gpu_workaround`, `#[cfg(target_os = "linux")]`) sets it
before `tauri::Builder::build` when **both** hold:

1. the user has not set `WEBKIT_DISABLE_DMABUF_RENDERER` at all (any value wins), and
2. no `/dev/dri/renderD*` node opens (absent `/dev/dri`, or every render node unopenable).

Rendering defaults are otherwise untouched. Ceiling: a render node that opens but
has no Mesa driver still logs the EGL warnings and is not caught; set the variable
by hand there.

## Not verified from the Mac

The Linux and Windows apps were not run. Coverage is `src/lib/platform.test.ts`
(UA → container per shell), `src/link/harnesses.test.ts` (local row, default
`active`, no pairing on each shell; a browser needs pairing) and reading; the
Linux branch of `lib.rs` does not compile on macOS and no Linux target is installed.
