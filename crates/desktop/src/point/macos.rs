//! Overlay windows as non-activating `NSPanel`s (96 §4.5): popup-menu level, every Space, never
//! ordered out — concealed is alpha 0 + mouse-transparent, like the island (`island/macos.rs`).
//! In `annotate = mouse` a revealed overlay catches the mouse instead, so the press that draws never
//! reaches the app underneath; the panel stays non-activating and never takes key.

use anyhow::Context;
use objc2_app_kit::NSEvent;
use tauri::WebviewWindow;
#[allow(deprecated)]
use tauri_nspanel::cocoa::appkit::NSWindowCollectionBehavior;
use tauri_nspanel::{ManagerExt, WebviewWindowExt};

/// `NSWindowStyleMaskNonactivatingPanel` alone (borderless; a trail is never resized).
const STYLE_MASK: i32 = 1 << 7;
/// `NSPopUpMenuWindowLevel`: above the island's status level 25 and menus, below the screen saver
/// and lock screen (§0.4).
const POPUP_MENU_LEVEL: i32 = 101;

/// Swizzle a freshly built (invisible) overlay into a panel, show it once, conceal it. Main thread.
// `tauri-nspanel` speaks the old `cocoa` types; its `set_collection_behaviour` takes nothing else.
#[allow(deprecated)]
pub fn panelize(win: &WebviewWindow) -> anyhow::Result<()> {
    let panel = win.to_panel().context("overlay to_panel")?;
    panel.set_style_mask(STYLE_MASK);
    panel.set_level(POPUP_MENU_LEVEL);
    panel.set_collection_behaviour(
        NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorIgnoresCycle,
    );
    panel.set_hides_on_deactivate(false);
    // A canvas never asks for key: a caught click must not move keyboard focus (the island's rule,
    // `island/macos.rs:49`).
    panel.set_becomes_key_only_if_needed(true);
    // Shown once, then only alpha: an ordered-out WKWebView is throttled (`island/macos.rs:51-54`).
    win.show().context("overlay initial show")?;
    panel.set_alpha_value(0.0);
    panel.set_ignore_mouse_events(true);
    Ok(())
}

/// Alpha 1 (front; click-through unless `catch_mouse`) or alpha 0 (click-through again, restored
/// before the alpha drops). Main thread.
pub fn set_revealed(win: &WebviewWindow, on: bool, catch_mouse: bool) -> anyhow::Result<()> {
    let panel = win
        .get_webview_panel(win.label())
        .map_err(|e| anyhow::anyhow!("panel `{}`: {e:?}", win.label()))?;
    panel.set_ignore_mouse_events(!(on && catch_mouse));
    panel.set_alpha_value(if on { 1.0 } else { 0.0 });
    if on {
        panel.order_front_regardless();
    }
    Ok(())
}

/// Left button held right now, any app (`NSEvent.pressedMouseButtons` bit 0; no TCC permission).
pub fn left_button_down() -> bool {
    NSEvent::pressedMouseButtons() & 1 != 0
}
