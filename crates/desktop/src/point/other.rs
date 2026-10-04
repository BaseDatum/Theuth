//! Windows and Linux overlays: Tauri's cross-platform flags only (transparent, undecorated,
//! always-on-top, click-through, built hidden). WebView2 and WebKitGTK keep a hidden webview's
//! scripts running, so concealed is a plain `hide()`; the window-system rows of 58 §6.2 are open (96 §12).

use tauri::WebviewWindow;

/// Nothing to swizzle: the builder's flags are the whole story here.
pub fn panelize(_win: &WebviewWindow) -> anyhow::Result<()> {
    Ok(())
}

pub fn set_revealed(win: &WebviewWindow, on: bool) -> anyhow::Result<()> {
    if on {
        win.show()?;
    } else {
        win.hide()?;
    }
    Ok(())
}
