//! Design doc 25: a macOS menubar app over the RPC WebSocket. The Rust side does
//! launch, probe, quit, attachment writes and OS integration only (25 §0.4);
//! the webview owns the WebSocket. One hidden-on-close window, a template tray
//! icon that toggles it, `ActivationPolicy::Accessory` (no Dock icon).

mod commands;
mod launch;

use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::Duration;

use anyhow::Context;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewWindow, WindowEvent, Wry};
use tauri_plugin_positioner::{Position, WindowExt};

/// The webview acknowledges `quit-requested` with `quit_ack` (after RPC
/// `shutdown` when it owns the daemon); this is the fallback.
const QUIT_ACK_TIMEOUT: Duration = Duration::from_secs(3);

/// One-shot flags: position the window under the tray on the first show of a
/// fresh profile (window-state restores it afterwards); run the quit handshake once.
#[derive(Default)]
struct Flags {
    position_on_show: AtomicBool,
    quitting: AtomicBool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(launch::Launch::default())
        .manage(Flags::default())
        .invoke_handler(tauri::generate_handler![
            commands::connection,
            commands::launch_log,
            commands::relaunch,
            commands::save_attachment,
            commands::quit_ack,
        ])
        .menu(app_menu)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                log_err(window.hide(), "hide window");
            }
        })
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            setup(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            app.state::<launch::Launch>().shutdown();
        }
    });
    Ok(())
}

/// Tray menu + click handler on the tray the config created; first-show flag.
fn setup(app: &AppHandle) -> anyhow::Result<()> {
    app.asset_protocol_scope()
        .allow_directory(launch::data_dir(), true)
        .context("allow WTH_DATA_DIR in asset protocol")?;
    let state_file = app
        .path()
        .app_config_dir()?
        .join(tauri_plugin_window_state::DEFAULT_FILENAME);
    app.state::<Flags>()
        .position_on_show
        .store(!state_file.exists(), SeqCst);

    let tray = app.tray_by_id("main").context("tray icon `main`")?;
    tray.set_menu(Some(Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "open", "Open", true, None::<&str>)?,
            &MenuItem::with_id(app, "new", "New chat", true, None::<&str>)?,
            &MenuItem::with_id(app, "knowledge", "Knowledge", true, None::<&str>)?,
            &MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
        ],
    )?))?;
    tray.on_tray_icon_event(|tray, event| {
        tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            toggle(tray.app_handle());
        }
    });
    Ok(())
}

/// Tauri's default macOS menu minus About, with Quit as our own item so ⌘Q goes
/// through the `quit-requested` handshake instead of `terminate:` (25 §4.5:
/// the menu stays so ⌘C/⌘V/⌘A/⌘W reach the webview under Accessory policy).
fn app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                &app.package_info().name,
                true,
                &[
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::show_all(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "quit", "Quit", true, Some("CmdOrCtrl+Q"))?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &Submenu::with_items(
                app,
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(app, None)?,
                    &PredefinedMenuItem::maximize(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::close_window(app, None)?,
                ],
            )?,
        ],
    )
}

/// Shared by the tray menu and the app menu (same ids).
fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => show(app),
        "new" => navigate(app, "/"),
        "knowledge" => navigate(app, "/knowledge"),
        "settings" => navigate(app, "/settings/general"),
        "quit" => request_quit(app),
        _ => {}
    }
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

fn toggle(app: &AppHandle) {
    match main_window(app) {
        Some(win) if win.is_visible().unwrap_or(false) => log_err(win.hide(), "hide window"),
        _ => show(app),
    }
}

fn show(app: &AppHandle) {
    let Some(win) = main_window(app) else {
        return;
    };
    if app.state::<Flags>().position_on_show.swap(false, SeqCst) {
        log_err(win.move_window(Position::TrayCenter), "position window");
    }
    log_err(win.show(), "show window");
    log_err(win.set_focus(), "focus window");
}

/// Show the window and tell the router where to go.
fn navigate(app: &AppHandle, route: &str) {
    show(app);
    log_err(app.emit("navigate", route), "emit navigate");
}

/// 25 §3 `/quit`: ask the webview to run RPC `shutdown` (if it owns the daemon)
/// and `quit_ack`; exit anyway after [`QUIT_ACK_TIMEOUT`]. The pty hang-up runs
/// in [`RunEvent::Exit`].
fn request_quit(app: &AppHandle) {
    if app.state::<Flags>().quitting.swap(true, SeqCst) {
        return;
    }
    if app.emit("quit-requested", ()).is_err() {
        app.exit(0);
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(QUIT_ACK_TIMEOUT);
        app.exit(0);
    });
}

fn log_err<T, E: std::fmt::Display>(r: Result<T, E>, what: &str) {
    if let Err(e) = r {
        tracing::warn!(error = %e, "{what}");
    }
}
