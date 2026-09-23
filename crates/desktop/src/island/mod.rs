//! The meetings island across window systems (58 §6.2): one Tauri command set —
//! `island_show/hide/place/resign_key`, `set_hidden_from_capture`, `island_capabilities` — with a
//! backend per OS. macOS is the as-built `NSPanel` module, moved verbatim; Windows and Linux share
//! the generic commands here and differ only in [`backend::init`] / [`backend::show`]. Where an OS
//! cannot do a thing the command is a no-op `Ok` and [`island_capabilities`] says so, so the UI can
//! be honest (58 §3.3) instead of the frontend guessing from `Container`.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as backend;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as backend;

/// What this OS's island can do; the webview reads it once.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Capabilities {
    /// `set_hidden_from_capture` really excludes both windows from screen shares.
    pub hide_from_share: bool,
    /// The island stays above every other window.
    pub always_on_top: bool,
    /// A click on the island never takes focus from the meeting app.
    pub non_activating: bool,
    /// Default placement: `"notch"` (macOS), `"top-centre"` of the chosen monitor's work area
    /// (Windows), or `"user"` (Linux: the dragged position is all we remember).
    pub placement: &'static str,
}

/// 58 §6.2, one row per window system.
#[tauri::command]
pub fn island_capabilities() -> Capabilities {
    #[cfg(target_os = "macos")]
    return Capabilities { hide_from_share: true, always_on_top: true, non_activating: true, placement: "notch" };
    #[cfg(target_os = "windows")]
    return Capabilities { hide_from_share: true, always_on_top: true, non_activating: true, placement: "top-centre" };
    #[cfg(target_os = "linux")]
    return Capabilities {
        hide_from_share: false,
        always_on_top: true,
        non_activating: false,
        placement: "user",
    };
}

#[cfg(not(target_os = "macos"))]
pub use generic::*;

/// Windows and Linux: Tauri's cross-platform window API for size, visibility and monitors; the
/// backend adds the window-system hooks. Sizes mirror `macos.rs` (03 §2).
#[cfg(not(target_os = "macos"))]
mod generic {
    use std::collections::HashMap;

    use anyhow::Context;
    use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Monitor, PhysicalPosition, WebviewWindow};

    use super::backend;

    pub(super) const LABEL: &str = "island";
    const WIDTH: f64 = 440.0;
    const MIN_WIDTH: f64 = 300.0;
    const MAX_WIDTH: f64 = 1200.0;
    const MIN_HEIGHT: f64 = 44.0;
    const MAX_HEIGHT: f64 = 800.0;
    /// Topology key → name of the monitor the default placement chose (the one under the cursor at
    /// first show), beside the window-state plugin's file in `app_config_dir`.
    const MONITORS_FILE: &str = "island-monitors.json";

    /// Window-system hooks on the island window, then hidden from capture where the OS can
    /// (`set_content_protected` is `WDA_EXCLUDEFROMCAPTURE` on Windows; Linux has nothing). Runs in
    /// `setup`, on the main thread.
    pub fn init(app: &AppHandle) -> anyhow::Result<()> {
        let win = app.get_webview_window(LABEL).context("window `island`")?;
        backend::init(&win)?;
        if super::island_capabilities().hide_from_share {
            set_hidden(app, true)?;
        }
        Ok(())
    }

    /// Both windows `set_content_protected(hidden)`; a no-op where the OS cannot (Linux). Every
    /// webview hears `capture-hidden` so the island's eye and the Meetings tab agree.
    #[tauri::command]
    pub fn set_hidden_from_capture(app: AppHandle, hidden: bool) -> Result<(), String> {
        if super::island_capabilities().hide_from_share {
            set_hidden(&app, hidden).map_err(|e| format!("{e:#}"))?;
        }
        app.emit("capture-hidden", hidden).map_err(|e| e.to_string())
    }

    /// Size to `width × height` logical px (clamped) with the top-left fixed, then show without
    /// taking focus (the backend's).
    #[tauri::command]
    pub fn island_show(app: AppHandle, width: f64, height: f64) -> Result<(), String> {
        let win = island(&app)?;
        let top_left = win.outer_position().map_err(|e| e.to_string())?;
        let size = LogicalSize::new(width.clamp(MIN_WIDTH, MAX_WIDTH), height.clamp(MIN_HEIGHT, MAX_HEIGHT));
        win.set_size(size).map_err(|e| e.to_string())?;
        win.set_position(top_left).map_err(|e| e.to_string())?;
        backend::show(&win).map_err(|e| format!("{e:#}"))
    }

    /// No key window to resign: on Windows the island never activates (`WS_EX_NOACTIVATE`); on Linux
    /// the window manager owns focus.
    #[tauri::command]
    pub fn island_resign_key(_app: AppHandle) -> Result<(), String> {
        Ok(())
    }

    /// Plain hide (`ShowWindow(SW_HIDE)` / `gtk_widget_hide`). WebView2 and WebKitGTK keep a hidden
    /// webview's scripts and sockets running, so the macOS alpha-0 trick is not needed.
    #[tauri::command]
    pub fn island_hide(app: AppHandle) -> Result<(), String> {
        island(&app)?.hide().map_err(|e| e.to_string())
    }

    /// Place the island for the current monitor topology and answer `(x, y, key)` in logical,
    /// top-left coordinates. `positions[key]` (the webview's per-topology memory of a drag) wins;
    /// else top-centre of the work area of the remembered monitor for this topology, else of the
    /// monitor under the cursor (remembered from then on), else the primary.
    #[tauri::command]
    pub fn island_place(
        app: AppHandle,
        positions: HashMap<String, (f64, f64)>,
    ) -> Result<(f64, f64, String), String> {
        let monitors = app.available_monitors().map_err(|e| e.to_string())?;
        let mut key: Vec<String> = monitors.iter().map(monitor_key).collect();
        key.sort();
        let key = key.join(";");
        let win = island(&app)?;
        let (x, y) = match positions.get(&key) {
            Some(&p) => {
                win.set_position(LogicalPosition::new(p.0, p.1)).map_err(|e| e.to_string())?;
                p
            }
            None => {
                let mut remembered = load_monitors(&app);
                let monitor = remembered
                    .get(&key)
                    .and_then(|name| monitors.iter().find(|m| m.name() == Some(name)))
                    .cloned()
                    .or_else(|| monitor_under_cursor(&app))
                    .or_else(|| app.primary_monitor().ok().flatten())
                    .or_else(|| monitors.first().cloned())
                    .ok_or("no monitors")?;
                if let Some(name) = monitor.name() {
                    if remembered.insert(key.clone(), name.clone()) != Some(name.clone()) {
                        save_monitors(&app, &remembered);
                    }
                }
                let scale = monitor.scale_factor();
                let wa = monitor.work_area();
                // ponytail: one scale for the whole monitor; mixed-DPI drags across monitors save the
                // position in the island's own scale, which is what the webview reads back anyway.
                let px = wa.position.x as f64 + (wa.size.width as f64 - WIDTH * scale) / 2.0;
                let py = wa.position.y as f64;
                win.set_position(PhysicalPosition::new(px.round() as i32, py))
                    .map_err(|e| e.to_string())?;
                (px / scale, py as f64 / scale)
            }
        };
        Ok((x, y, key))
    }

    pub(super) fn island(app: &AppHandle) -> Result<WebviewWindow, String> {
        app.get_webview_window(LABEL).ok_or_else(|| "window `island`".into())
    }

    fn set_hidden(app: &AppHandle, hidden: bool) -> anyhow::Result<()> {
        for label in ["main", LABEL] {
            app.get_webview_window(label)
                .with_context(|| format!("window `{label}`"))?
                .set_content_protected(hidden)
                .with_context(|| format!("set_content_protected on `{label}`"))?;
        }
        Ok(())
    }

    fn monitor_key(m: &Monitor) -> String {
        let p = m.position();
        let s = m.size();
        format!("{}:{},{},{},{}", m.name().map_or("", String::as_str), p.x, p.y, s.width, s.height)
    }

    /// The monitor under the cursor; `None` where the cursor cannot be read (Wayland).
    fn monitor_under_cursor(app: &AppHandle) -> Option<Monitor> {
        let c = app.cursor_position().ok()?;
        app.monitor_from_point(c.x, c.y).ok().flatten()
    }

    fn monitors_file(app: &AppHandle) -> Option<std::path::PathBuf> {
        app.path().app_config_dir().ok().map(|d| d.join(MONITORS_FILE))
    }

    fn load_monitors(app: &AppHandle) -> HashMap<String, String> {
        monitors_file(app)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_monitors(app: &AppHandle, m: &HashMap<String, String>) {
        let Some(path) = monitors_file(app) else { return };
        let write = || -> anyhow::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, serde_json::to_vec(m)?)?;
            Ok(())
        };
        if let Err(e) = write() {
            tracing::warn!(error = %e, path = %path.display(), "save island monitors");
        }
    }
}
