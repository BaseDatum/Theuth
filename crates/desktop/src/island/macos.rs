//! The meetings island (53 decisions 8–9, `meetings/03-island.md` §1): the
//! `island` window swizzled into a non-activating `NSPanel` at status level on
//! every Space, capture exclusion for both windows, and placement on the notch.
//! OS integration only — what shows in the panel is the webview's (`/island`).

use std::ptr::NonNull;

use anyhow::Context;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplicationDidChangeScreenParametersNotification, NSScreen};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSRect};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewWindow};
#[allow(deprecated)]
use tauri_nspanel::cocoa::appkit::NSWindowCollectionBehavior;
use tauri_nspanel::{ManagerExt, WebviewWindowExt};

const LABEL: &str = "island";
/// Default width (03 §2); the user may drag the edges.
const WIDTH: f64 = 440.0;
const MIN_WIDTH: f64 = 300.0;
const MAX_WIDTH: f64 = 1200.0;
/// Strip only (03 §2); the brief pane grows the window to its content, capped, until the user sizes it.
const MIN_HEIGHT: f64 = 44.0;
const MAX_HEIGHT: f64 = 800.0;
/// `NSWindowStyleMaskNonactivatingPanel | NSWindowStyleMaskResizable` (borderless otherwise): a click never
/// activates the app; the edges resize.
const STYLE_MASK: i32 = (1 << 7) | (1 << 3);
/// `NSStatusWindowLevel`: above the menu bar and full-screen apps' auxiliary layer.
const STATUS_LEVEL: i32 = 25;

/// Swizzle the island into a panel and hide both windows from capture (decision 9's default);
/// the webview re-applies its stored eye setting on boot. Runs in `setup`, on the main thread.
// `tauri-nspanel` speaks the old `cocoa` types; its `set_collection_behaviour` takes nothing else.
#[allow(deprecated)]
pub fn init(app: &AppHandle) -> anyhow::Result<()> {
    let win = app.get_webview_window(LABEL).context("window `island`")?;
    let panel = win.to_panel().context("island to_panel")?;
    panel.set_style_mask(STYLE_MASK);
    panel.set_level(STATUS_LEVEL);
    panel.set_collection_behaviour(
        NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary,
    );
    // Panels hide when the app deactivates by default; ours is an Accessory app that is rarely active.
    panel.set_hides_on_deactivate(false);
    // Buttons never take key, so typing keeps going to the meeting app (check 4); only the request box
    // does, and `island_resign_key` hands it back.
    panel.set_becomes_key_only_if_needed(true);
    set_hidden(app, true)?;
    // The window is never ordered out: WebKit throttles a webview whose window is not on screen until
    // its RPC client never connects (03 §9). "Hidden" is alpha 0 + mouse-transparent, ordered in.
    win.show().context("island initial show")?;
    conceal(&panel);
    watch_screens(app);
    Ok(())
}

fn conceal(panel: &tauri_nspanel::Panel) {
    panel.set_alpha_value(0.0);
    panel.set_ignore_mouse_events(true);
}

/// Both windows `set_content_protected(hidden)` (`NSWindow.sharingType = .none`); every webview
/// hears `capture-hidden` so the island's eye and the Meetings tab agree.
#[tauri::command]
pub fn set_hidden_from_capture(app: AppHandle, hidden: bool) -> Result<(), String> {
    set_hidden(&app, hidden).map_err(|e| format!("{e:#}"))?;
    app.emit("capture-hidden", hidden)
        .map_err(|e| e.to_string())
}

/// Order the island front without focusing anything at `width × height` logical px (the webview's measured
/// content, or the size the user dragged), top-left fixed.
#[tauri::command]
pub fn island_show(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
    let win = island(&app)?;
    let top_left = win.outer_position().map_err(|e| e.to_string())?;
    let size = LogicalSize::new(
        width.clamp(MIN_WIDTH, MAX_WIDTH),
        height.clamp(MIN_HEIGHT, MAX_HEIGHT),
    );
    win.set_size(size).map_err(|e| e.to_string())?;
    win.set_position(top_left).map_err(|e| e.to_string())?;
    let panel = app.get_webview_panel(LABEL).map_err(|e| format!("{e:?}"))?;
    panel.set_alpha_value(1.0);
    panel.set_ignore_mouse_events(false);
    panel.order_front_regardless();
    Ok(())
}

/// The request box lost focus: give the key window back to the meeting app. Typing into the box made
/// the non-activating panel key (`becomesKeyOnlyIfNeeded`); nothing else in the island ever does.
#[tauri::command]
pub fn island_resign_key(app: AppHandle) -> Result<(), String> {
    app.get_webview_panel(LABEL)
        .map_err(|e| format!("{e:?}"))?
        .resign_key_window();
    Ok(())
}

/// Conceal, never `orderOut` (see `init`).
#[tauri::command]
pub fn island_hide(app: AppHandle) -> Result<(), String> {
    conceal(&app.get_webview_panel(LABEL).map_err(|e| format!("{e:?}"))?);
    Ok(())
}

/// Place the island for the current screen configuration and answer `(x, y, key)` in logical,
/// top-left coordinates. `positions[key]` (the webview's per-configuration memory) wins; else the
/// default: centred on the notch of a notched screen with the top at the screen top (content under
/// the bezel is hidden — 53 §2.3), or centred just under the menu bar of the primary screen.
#[tauri::command]
pub fn island_place(
    app: AppHandle,
    positions: std::collections::HashMap<String, (f64, f64)>,
) -> Result<(f64, f64, String), String> {
    // Sync commands run on the main thread, which `NSScreen` requires.
    let mtm = MainThreadMarker::new().ok_or("island_place off the main thread")?;
    let screens = NSScreen::screens(mtm);
    let primary = screens.firstObject().ok_or("no screens")?;
    let primary_h = primary.frame().size.height;
    let mut key: Vec<String> = screens.iter().map(|s| rect_key(s.frame())).collect();
    key.sort();
    let key = key.join(";");
    let (x, y) = match positions.get(&key) {
        Some(&p) => p,
        None => match screens.iter().find(|s| s.safeAreaInsets().top > 0.0) {
            Some(notched) => {
                let f = notched.frame();
                (
                    f.origin.x + (f.size.width - WIDTH) / 2.0,
                    primary_h - (f.origin.y + f.size.height),
                )
            }
            None => {
                let f = primary.frame();
                let v = primary.visibleFrame();
                (
                    f.origin.x + (f.size.width - WIDTH) / 2.0,
                    primary_h - (v.origin.y + v.size.height),
                )
            }
        },
    };
    island(&app)?
        .set_position(LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    Ok((x, y, key))
}

fn island(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window(LABEL)
        .ok_or_else(|| "window `island`".into())
}

/// Every window, so a popout opened later is covered by the next toggle (`open_window`'s webview re-applies on boot).
fn set_hidden(app: &AppHandle, hidden: bool) -> anyhow::Result<()> {
    for (label, win) in app.webview_windows() {
        win.set_content_protected(hidden)
            .with_context(|| format!("set_content_protected on `{label}`"))?;
    }
    Ok(())
}

fn rect_key(r: NSRect) -> String {
    format!(
        "{},{},{},{}",
        r.origin.x, r.origin.y, r.size.width, r.size.height
    )
}

/// `NSApplicationDidChangeScreenParametersNotification` → `screens-changed`; the island webview
/// re-calls `island_place`. The centre holds the observer for the life of the process.
fn watch_screens(app: &AppHandle) {
    let app = app.clone();
    let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
        if let Err(e) = app.emit("screens-changed", ()) {
            tracing::warn!(error = %e, "emit screens-changed");
        }
    });
    // SAFETY: the name is AppKit's static; no object filter, no queue (the block runs on the posting
    // thread — the main thread for this notification); the block owns only a `Send` `AppHandle`.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(token);
}
