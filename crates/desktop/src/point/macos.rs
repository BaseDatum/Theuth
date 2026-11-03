//! Overlay windows as non-activating `NSPanel`s (96 §4.5): popup-menu level, every Space, never
//! ordered out — concealed is alpha 0 + mouse-transparent, like the island (`island/macos.rs`).
//! In `annotate = mouse` a revealed overlay catches the mouse instead, so the press that draws never
//! reaches the app underneath; the panel stays non-activating and never takes key.

use anyhow::Context;
use objc2_app_kit::NSEvent;
use tauri::WebviewWindow;
use tauri_nspanel::panel::{NSWindowCollectionBehavior, NSWindowStyleMask};
use tauri_nspanel::{ManagerExt, WebviewWindowExt};

use crate::island::{PanelSpec, TRACKING};

const SPEC: PanelSpec = PanelSpec {
    can_become_key_window: true,
    // `NSWindowStyleMaskNonactivatingPanel` alone (borderless; a trail is never resized).
    style_mask: NSWindowStyleMask::NonactivatingPanel,
    // `NSPopUpMenuWindowLevel`: above the island's status level 25 and menus, below the screen saver
    // and lock screen (§0.4).
    level: 101,
    collection_behavior: NSWindowCollectionBehavior::CanJoinAllSpaces
        .union(NSWindowCollectionBehavior::FullScreenAuxiliary)
        .union(NSWindowCollectionBehavior::Stationary)
        .union(NSWindowCollectionBehavior::IgnoresCycle),
    hides_on_deactivate: false,
    // A canvas never asks for key: a caught click must not move keyboard focus (the island's rule,
    // `island/macos.rs` `SPEC`).
    becomes_key_only_if_needed: true,
};

/// The overlays' `NSPanel` subclass; its own module because `tauri_panel!` expands `use`s.
mod class {
    tauri_nspanel::tauri_panel!(TheuthOverlayPanel {
        config: {
            can_become_key_window: super::SPEC.can_become_key_window
        }
        with: {
            tracking_area: {
                options: super::TRACKING,
                auto_resize: false
            }
        }
    });
}

/// Swizzle a freshly built (invisible) overlay into a panel, show it once, conceal it. Main thread.
pub fn panelize(win: &WebviewWindow) -> anyhow::Result<()> {
    let panel = win
        .to_panel::<class::TheuthOverlayPanel>()
        .context("overlay to_panel")?;
    SPEC.apply(&*panel)?;
    // Shown once, then only alpha: an ordered-out WKWebView is throttled (`island/macos.rs` `init`).
    win.show().context("overlay initial show")?;
    panel.set_alpha_value(0.0);
    panel.set_ignores_mouse_events(true);
    Ok(())
}

/// Alpha 1 (front; click-through unless `catch_mouse`) or alpha 0 (click-through again, restored
/// before the alpha drops). Main thread.
pub fn set_revealed(win: &WebviewWindow, on: bool, catch_mouse: bool) -> anyhow::Result<()> {
    let panel = win
        .get_webview_panel(win.label())
        .map_err(|e| anyhow::anyhow!("panel `{}`: {e:?}", win.label()))?;
    panel.set_ignores_mouse_events(!(on && catch_mouse));
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers tauri-nspanel 2.0 sent for an overlay (`STYLE_MASK`, `POPUP_MENU_LEVEL` and the
    /// `cocoa` behaviour bits before 174).
    #[test]
    fn overlay_spec_is_the_pre_port_panel() {
        let pre_port = PanelSpec {
            can_become_key_window: true, // RawNSPanel's canBecomeKeyWindow: YES
            style_mask: NSWindowStyleMask(128), // NSWindowStyleMaskNonactivatingPanel (1 << 7)
            level: 101,                  // NSPopUpMenuWindowLevel
            // NSWindowCollectionBehaviorCanJoinAllSpaces (1 << 0) | …Stationary (1 << 4)
            // | …IgnoresCycle (1 << 6) | …FullScreenAuxiliary (1 << 8)
            collection_behavior: NSWindowCollectionBehavior(1 | 16 | 64 | 256),
            hides_on_deactivate: false,
            becomes_key_only_if_needed: true,
        };
        assert_eq!(SPEC, pre_port);
    }
}
