//! Design docs 25 and 31 §8.3. The Rust side does launch, probe, quit and OS
//! integration only (25 §0.4); the webview owns the WebSocket. Desktop: one
//! hidden-on-close window, a template tray icon that toggles it,
//! `ActivationPolicy::Accessory` (no Dock icon), the local daemon on a pty.
//! Mobile: the same webview as a link device (QR scanner + universal link), no
//! local daemon; the window is built here (not from `tauri.ios.conf.json`) so the
//! iOS keyboard's accessory bar can be dropped, and backgrounding holds a
//! `UIBackgroundTask` so the relay socket survives the grace period. `main.rs`
//! and the generated iOS bridge both enter at [`run`].

mod commands;
#[cfg(desktop)]
mod island;
#[cfg(target_os = "ios")]
mod ios_windowing;
#[cfg(desktop)]
mod launch;
#[cfg(desktop)]
mod models;
#[cfg(desktop)]
mod point;
#[cfg(desktop)]
mod update;

/// Build and run the app. Desktop and mobile share the opener/notification
/// plugins; everything else is one `cfg` side or the other.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt::init();
    #[cfg(target_os = "linux")]
    desktop::webkit_no_gpu_workaround();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init());
    #[cfg(desktop)]
    let builder = desktop::configure(builder);
    #[cfg(mobile)]
    let builder = mobile::configure(builder);
    let app = builder
        .build(tauri::generate_context!())
        .expect("build tauri app");
    #[cfg(target_os = "ios")]
    ios_windowing::install();
    #[cfg(desktop)]
    app.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            use tauri::Manager;
            // 39 §3.3: stop the sidecar first, then swap the bundle it ran from.
            app.state::<launch::Launch>().shutdown();
            update::install_pending(app);
        }
    });
    #[cfg(mobile)]
    app.run(mobile::on_event);
}

/// `--bg-page` from `ui/src/styles.css` as the native window + webview background, so the frames before the
/// page's first paint are the page colour instead of WKWebView's opaque white (DARK-FLASH). `dark` is the
/// system appearance at the call; once painted, the page follows `prefers-color-scheme` itself.
fn page_background(dark: bool) -> tauri::window::Color {
    if dark {
        tauri::window::Color(0x12, 0x10, 0x0e, 0xff)
    } else {
        tauri::window::Color(0xe8, 0xdc, 0xc8, 0xff)
    }
}

/// The phone (31 §8): plugins, the one window, and the background grace period.
#[cfg(mobile)]
mod mobile {
    use tauri::{AppHandle, Builder, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent, Wry};

    use crate::commands;

    pub(super) fn configure(builder: Builder<Wry>) -> Builder<Wry> {
        builder
            .plugin(tauri_plugin_barcode_scanner::init())
            .plugin(tauri_plugin_deep_link::init())
            .plugin(tauri_plugin_haptics::init())
            .invoke_handler(tauri::generate_handler![commands::connection])
            .setup(|app| {
                let builder =
                    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                        .background_color(crate::page_background(system_dark()));
                // WKWebView paints a native "‹ › Done" bar above the keyboard for every
                // focused input (and paints it light under a dark page); a chat composer
                // has no use for it.
                #[cfg(target_os = "ios")]
                let builder = builder.with_input_accessory_view_builder(|_| None);
                builder.build()?;
                Ok(())
            })
    }

    /// tao's iOS `Window::theme` is a constant `Light`; ask UIKit for the screen's appearance instead.
    fn system_dark() -> bool {
        #[cfg(target_os = "ios")]
        {
            use objc2::MainThreadMarker;
            use objc2_ui_kit::{UIScreen, UITraitEnvironment, UIUserInterfaceStyle};
            #[allow(deprecated)]
            return MainThreadMarker::new().is_some_and(|mtm| {
                let style = unsafe {
                    UIScreen::mainScreen(mtm)
                        .traitCollection()
                        .userInterfaceStyle()
                };
                style == UIUserInterfaceStyle::Dark
            });
        }
        #[cfg(not(target_os = "ios"))]
        false
    }

    /// Legacy lifecycle: `Suspended` is `applicationWillResignActive`, `Resumed` is `willEnterForeground`.
    /// Scene lifecycle (`ios_windowing`): tao 0.35 emits neither and reports `sceneWillResignActive` /
    /// `sceneDidBecomeActive` as `Focused`, so both pairs drive the background task.
    #[cfg(target_os = "ios")]
    pub(super) fn on_event(app: &AppHandle, event: RunEvent) {
        match event {
            RunEvent::SceneRequested { scene, .. } => crate::ios_windowing::dismiss(app, &scene),
            RunEvent::WindowEvent { event: WindowEvent::Suspended | WindowEvent::Focused(false), .. } => {
                background::hold()
            }
            RunEvent::WindowEvent { event: WindowEvent::Resumed | WindowEvent::Focused(true), .. } => {
                background::release()
            }
            _ => {}
        }
    }

    #[cfg(not(target_os = "ios"))]
    pub(super) fn on_event(_: &AppHandle, _: RunEvent) {}

    /// iOS suspends the process seconds after the app leaves the screen, and the
    /// relay socket dies with it. A background task buys the ~30 s iOS grants, so
    /// a turn that finishes right after the user switches away still streams in
    /// and posts its notification (31 §8). Anything longer needs push, which the
    /// relay cannot do without seeing plaintext.
    #[cfg(target_os = "ios")]
    mod background {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

        use objc2::MainThreadMarker;
        use objc2_ui_kit::UIApplication;

        /// No task in flight. Real identifiers are small counters; `UIBackgroundTaskInvalid` is 0.
        const NONE: usize = usize::MAX;
        static TASK: AtomicUsize = AtomicUsize::new(NONE);

        pub(super) fn hold() {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            end(mtm);
            // The handler must end the task or iOS kills the app; it runs on the main thread.
            let expired = block2::RcBlock::new(|| {
                if let Some(mtm) = MainThreadMarker::new() {
                    end(mtm);
                }
            });
            let id = UIApplication::sharedApplication(mtm)
                .beginBackgroundTaskWithExpirationHandler(Some(&expired));
            if id != unsafe { objc2_ui_kit::UIBackgroundTaskInvalid } {
                TASK.store(id, SeqCst);
            }
        }

        pub(super) fn release() {
            if let Some(mtm) = MainThreadMarker::new() {
                end(mtm);
            }
        }

        fn end(mtm: MainThreadMarker) {
            let id = TASK.swap(NONE, SeqCst);
            if id != NONE {
                UIApplication::sharedApplication(mtm).endBackgroundTask(id);
            }
        }
    }
}

/// The menubar app (25): tray, app menu, window show/hide, quit handshake and
/// the desktop-only plugins and commands.
#[cfg(desktop)]
mod desktop {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::time::Duration;

    use anyhow::Context;
    #[cfg(target_os = "macos")]
    use tauri::menu::Submenu;
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
    use tauri::{AppHandle, Builder, Emitter, Manager, WebviewWindow, WindowEvent, Wry};
    use tauri_plugin_positioner::{Position, WindowExt};

    use crate::{commands, island, launch, models, point, update};

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

    /// WebKitGTK's DMA-BUF renderer needs a DRM render node; without one (a VM with no GPU
    /// passthrough) it logs `libEGL warning: failed to get driver name` / `ZINK: failed to
    /// choose pdev` and may paint a blank window. `WEBKIT_DISABLE_DMABUF_RENDERER=1` is the
    /// upstream workaround; set it only when the user has not decided and no render node
    /// opens, before GTK/WebKit initialise (`tauri::Builder::build`).
    #[cfg(target_os = "linux")]
    pub(super) fn webkit_no_gpu_workaround() {
        const VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
        if std::env::var_os(VAR).is_some() {
            return;
        }
        let usable = std::fs::read_dir("/dev/dri")
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                e.file_name().to_string_lossy().starts_with("renderD")
                    && std::fs::File::open(e.path()).is_ok()
            });
        if usable {
            return;
        }
        // ponytail: a node that opens but has no Mesa driver still logs the EGL warnings; honour
        // WEBKIT_DISABLE_DMABUF_RENDERER by hand there, or probe EGL if that case turns up in the field.
        tracing::info!("no usable DRM render node; setting {VAR}=1 for WebKitGTK");
        // SAFETY: single-threaded here — before the Tauri builder spawns anything that reads the environment.
        unsafe { std::env::set_var(VAR, "1") };
    }

    /// Desktop plugins, managed state, commands, menu and window hooks.
    pub(super) fn configure(builder: Builder<Wry>) -> Builder<Wry> {
        let builder = builder
            .plugin(tauri_plugin_global_shortcut::Builder::new().build())
            .plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                None,
            ))
            .plugin(tauri_plugin_positioner::init())
            // The island is placed by `island::island_place`, never by saved window state (03 §1); the
            // overlays by `point::overlay_ensure` (96 §4.6). The denylist is exact-match, hence the filter.
            .plugin(
                tauri_plugin_window_state::Builder::new()
                    .with_denylist(&["island"])
                    .with_filter(|label| !label.starts_with(point::OVERLAY_PREFIX))
                    .build(),
            )
            .plugin(tauri_plugin_dialog::init())
            .plugin(update::plugin())
            .manage(launch::Launch::default())
            .manage(update::Pending::default())
            .manage(Flags::default());
        // 58 §6.2: one island command set on every desktop OS; the `NSPanel` plugin is macOS's.
        #[cfg(target_os = "macos")]
        let builder = builder.plugin(tauri_nspanel::init());
        let builder = builder.invoke_handler(tauri::generate_handler![
            commands::connection,
            commands::launch_log,
            commands::relaunch,
            commands::quit_ack,
            commands::open_window,
            update::check_update,
            update::install_update,
            island::set_hidden_from_capture,
            island::island_show,
            island::island_hide,
            island::island_place,
            island::island_resign_key,
            island::island_capabilities,
            point::voice_key_bind,
            point::point_cancel,
            point::point_done,
            point::point_state,
            point::point_refuse,
            point::point_configure,
            point::overlay_ensure,
        ]);
        // The native menu bar is macOS's (⌘C/⌘V under Accessory policy); GTK would paint it inside the window.
        #[cfg(target_os = "macos")]
        let builder = builder.menu(app_menu);
        builder
            .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
            .on_window_event(|window, event| match event {
                // Main and island hide on close (tray app); a popout is just a window and goes away.
                WindowEvent::CloseRequested { api, .. } => {
                    if window.label().starts_with(commands::POPOUT_PREFIX) {
                        return;
                    }
                    api.prevent_close();
                    log_err(window.hide(), "hide window");
                }
                WindowEvent::ThemeChanged(_) => {
                    if let Some(win) = window.app_handle().get_webview_window(window.label()) {
                        paint_page_background(&win);
                    }
                }
                _ => {}
            })
            .setup(|app| {
                #[cfg(target_os = "macos")]
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                setup(app.handle())?;
                island::init(app.handle())?;
                point::init(app.handle())?;
                Ok(())
            })
    }

    /// Asset scope, tray menu + click handler on the tray the config created;
    /// first-show flag.
    fn setup(app: &AppHandle) -> anyhow::Result<()> {
        app.asset_protocol_scope()
            .allow_directory(launch::data_dir(), true)
            .context("allow THEUTH_DATA_DIR in asset protocol")?;
        if let Some(win) = main_window(app) {
            paint_page_background(&win);
        }
        let state_file = app
            .path()
            .app_config_dir()?
            .join(tauri_plugin_window_state::DEFAULT_FILENAME);
        app.state::<Flags>()
            .position_on_show
            .store(!state_file.exists(), SeqCst);

        let tray = app.tray_by_id("main").context("tray icon `main`")?;
        let menu = Menu::with_items(
            app,
            &[
                &MenuItem::with_id(app, "open", "Open", true, None::<&str>)?,
                &MenuItem::with_id(app, "new", "New chat", true, None::<&str>)?,
                &MenuItem::with_id(app, "knowledge", "Knowledge", true, None::<&str>)?,
            ],
        )?;
        // 53 §4.1: the start form; the recorder is macOS only (Core Audio taps).
        if cfg!(target_os = "macos") {
            menu.append(&MenuItem::with_id(
                app,
                "meeting",
                "Start meeting…",
                true,
                None::<&str>,
            )?)?;
        }
        menu.append_items(&[
            &MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
        ])?;
        tray.set_menu(Some(menu))?;
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
        update::start(app);
        models::start(app);
        Ok(())
    }

    /// Tauri's default macOS menu minus About, with Quit as our own item so ⌘Q goes
    /// through the `quit-requested` handshake instead of `terminate:` (25 §4.5:
    /// the menu stays so ⌘C/⌘V/⌘A/⌘W reach the webview under Accessory policy).
    #[cfg(target_os = "macos")]
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
            "meeting" => navigate(app, "/meetings/new"),
            "settings" => navigate(app, "/settings/general"),
            "quit" => request_quit(app),
            _ => {}
        }
    }

    fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
        app.get_webview_window("main")
    }

    /// Native background = the page's, so no white frame before first paint (see [`crate::page_background`]).
    /// Not the island: it is transparent. Popouts get theirs from `open_window`; every window re-paints on
    /// `ThemeChanged`.
    pub(crate) fn paint_page_background(win: &WebviewWindow) {
        if win.label() == "island" || win.label().starts_with(point::OVERLAY_PREFIX) {
            return;
        }
        let dark = win.theme().is_ok_and(|t| t == tauri::Theme::Dark);
        log_err(
            win.set_background_color(Some(crate::page_background(dark))),
            "window background",
        );
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
    /// in `RunEvent::Exit` ([`crate::run`]).
    pub(crate) fn request_quit(app: &AppHandle) {
        if app.state::<Flags>().quitting.swap(true, SeqCst) {
            return;
        }
        if app.emit("quit-requested", ()).is_err() {
            update::exit(app);
            return;
        }
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(QUIT_ACK_TIMEOUT);
            update::exit(&app);
        });
    }

    fn log_err<T, E: std::fmt::Display>(r: Result<T, E>, what: &str) {
        if let Err(e) = r {
            tracing::warn!(error = %e, "{what}");
        }
    }
}
