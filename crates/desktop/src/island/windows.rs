//! Win32 hooks (58 §6.2 Windows row): the island `HWND` gets `WS_EX_TOOLWINDOW` (no taskbar/alt-tab
//! entry) | `WS_EX_NOACTIVATE` (a click never takes the foreground from the meeting app) |
//! `WS_EX_TOPMOST`, and shows with `SW_SHOWNOACTIVATE`. Capture exclusion is Tauri's
//! `set_content_protected` (`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` in tao), called from
//! `mod.rs`.

use std::ffi::c_void;

use anyhow::{anyhow, bail, Context};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tauri::WebviewWindow;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST,
};

const EX_STYLE: isize = (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST) as isize;

/// Add the three extended styles; runs in `setup` on the main (window) thread.
pub fn init(win: &WebviewWindow) -> anyhow::Result<()> {
    let hwnd = hwnd(win)?;
    // SAFETY: `hwnd` is the live island window owned by this thread's event loop.
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | EX_STYLE) == 0 && ex != 0 {
            bail!("SetWindowLongPtrW(GWL_EXSTYLE)");
        }
    }
    Ok(())
}

/// Show without activating, and re-assert topmost (a `SetWindowLongPtr` style change alone does not
/// reorder the window in the z-band).
pub fn show(win: &WebviewWindow) -> anyhow::Result<()> {
    let hwnd = hwnd(win)?;
    // SAFETY: as in `init`; the flags leave position and size alone.
    unsafe {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
    Ok(())
}

fn hwnd(win: &WebviewWindow) -> anyhow::Result<HWND> {
    match win.window_handle().context("island window handle")?.as_raw() {
        RawWindowHandle::Win32(h) => Ok(h.hwnd.get() as *mut c_void),
        other => Err(anyhow!("island window handle is not Win32: {other:?}")),
    }
}
