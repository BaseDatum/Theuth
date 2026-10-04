//! Windows and Linux overlays: Tauri's cross-platform flags only (transparent, undecorated,
//! always-on-top, click-through, built hidden). WebView2 and WebKitGTK keep a hidden webview's
//! scripts running, so concealed is a plain `hide()`; the window-system rows of 58 §6.2 are open (96 §12).

use tauri::WebviewWindow;

/// Nothing to swizzle: the builder's flags are the whole story here.
pub fn panelize(_win: &WebviewWindow) -> anyhow::Result<()> {
    Ok(())
}

/// Show (click-through unless `catch_mouse`) or hide (click-through restored first).
pub fn set_revealed(win: &WebviewWindow, on: bool, catch_mouse: bool) -> anyhow::Result<()> {
    win.set_ignore_cursor_events(!(on && catch_mouse))?;
    if on {
        win.show()?;
    } else {
        win.hide()?;
    }
    Ok(())
}

/// No global button reader here yet (96 §0.14): `annotate = mouse` draws nothing off macOS.
pub fn left_button_down() -> bool {
    false
}
