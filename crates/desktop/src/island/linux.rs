//! Linux (58 §6.2 X11 and Wayland rows). X11: EWMH `_NET_WM_STATE_ABOVE`, `_SKIP_TASKBAR`,
//! `_SKIP_PAGER` — GTK sends exactly those `_NET_WM_STATE` client messages for
//! `gtk_window_set_keep_above` / `set_skip_taskbar_hint` / `set_skip_pager_hint`, which is what
//! Tauri's `set_always_on_top` / `set_skip_taskbar` call in tao, so no `x11rb` round-trip on the
//! XID is needed. Wayland: the same calls are the "always on top best effort" — the compositor
//! decides, and positions are ignored. No capture hiding on either (capability `false`).

use anyhow::Context;
use tauri::WebviewWindow;

/// Re-assert the EWMH states the window config already asks for (`alwaysOnTop`, `skipTaskbar`) —
/// cheap, and it survives a window manager that only reads them after map.
pub fn init(win: &WebviewWindow) -> anyhow::Result<()> {
    win.set_always_on_top(true)
        .context("island set_always_on_top")?;
    win.set_skip_taskbar(true)
        .context("island set_skip_taskbar")?;
    Ok(())
}

/// `gtk_widget_show`; whether it takes focus is the window manager's call (58 §6.2: not
/// non-activating on Linux).
pub fn show(win: &WebviewWindow) -> anyhow::Result<()> {
    win.show().context("island show")
}

/// Display server and what Tauri reports, for the probe log.
pub fn describe(win: &WebviewWindow) -> String {
    let server = if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("GDK_BACKEND").ok().as_deref() != Some("x11")
    {
        "wayland"
    } else if std::env::var_os("DISPLAY").is_some() {
        "x11"
    } else {
        "none"
    };
    format!(
        "server={server} visible={:?} always_on_top={:?} focused={:?}",
        win.is_visible(),
        win.is_always_on_top(),
        win.is_focused()
    )
}
