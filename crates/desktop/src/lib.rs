//! Design docs 25 and 31 §8.3. The Rust side does launch, probe, quit and OS
//! integration only (25 §0.4); the webview owns the WebSocket. Desktop: one
//! hidden-on-close window, a template tray icon that toggles it, a Dock icon and
//! ⌘Tab entry (`ActivationPolicy::Regular`, 166: Dock click / ⌘Tab show the
//! hidden window, `dock.rs`), the local daemon on a pty.
//! Mobile: the same webview as a link device (QR scanner + universal link), no
//! local daemon; the window is built here (not from `tauri.ios.conf.json`) so the
//! iOS keyboard's accessory bar can be dropped (plus the iPad assistant bar,
//! `ios_keyboard.rs`), and backgrounding holds a
//! `UIBackgroundTask` so the relay socket survives the grace period. `main.rs`
//! and the generated iOS bridge both enter at [`run`].

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple_cookies;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple_tile;
#[cfg(desktop)]
mod asset;
mod commands;
#[cfg(desktop)]
mod companion;
#[cfg(target_os = "macos")]
mod dock;
#[cfg(target_os = "ios")]
mod ios_auth;
#[cfg(target_os = "ios")]
mod ios_engine;
#[cfg(target_os = "ios")]
mod ios_keyboard;
#[cfg(target_os = "ios")]
mod ios_webview;
#[cfg(target_os = "ios")]
mod ios_windowing;
#[cfg(desktop)]
mod island;
#[cfg(desktop)]
mod launch;
#[cfg(desktop)]
mod models;
mod pages;
#[cfg(desktop)]
mod point;
#[cfg(target_os = "macos")]
mod power;
#[cfg(desktop)]
mod quit;
#[cfg(desktop)]
mod update;
#[cfg(desktop)]
mod share;
#[cfg(desktop)]
mod sso;
#[cfg(any(desktop, target_os = "ios"))]
mod web_rules;
#[cfg(desktop)]
mod webview;

/// 218 §7: `wb_capture` where WebKit's `takeSnapshot` is not available (Linux, Windows, Android).
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
#[tauri::command]
async fn wb_capture(rect: serde_json::Value, width: Option<f64>) -> Result<(), String> {
    let _ = (rect, width);
    Err("wb_capture: unsupported".to_owned())
}

/// Build and run the app. Desktop and mobile share the opener/notification
/// plugins; everything else is one `cfg` side or the other.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The library crates that read the product version link on desktop and iOS (embedded engine).
    #[cfg(not(target_os = "android"))]
    platform::version::set(env!("CARGO_PKG_VERSION"));
    let _ = tracing_subscriber::fmt().try_init();
    #[cfg(target_os = "linux")]
    desktop::webkit_no_gpu_workaround();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        // 120 §4.2: inline sandboxed pages, on both containers.
        .register_uri_scheme_protocol(pages::SCHEME, pages::handler)
        .manage(pages::Pages::default());
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
    app.run(|app, event| match event {
        tauri::RunEvent::Exit => {
            // 209 §4.5: the tile store's cookie observer goes before WebKit does.
            #[cfg(target_os = "macos")]
            apple_cookies::unwatch(app);
            // 221 D20: the sleep / screen-lock observers too.
            #[cfg(target_os = "macos")]
            power::shutdown();
            quit::on_exit(app)
        }
        // 166 D2: a Dock click.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => dock::reopen(app),
        _ => {}
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
        let builder = builder
            .plugin(tauri_plugin_barcode_scanner::init())
            .plugin(tauri_plugin_deep_link::init())
            .plugin(tauri_plugin_haptics::init())
            // 218: Pencil double-tap/squeeze and handwriting recognition for the web whiteboard (iPadOS 17.5+).
            .plugin(tauri_plugin_ink::init());
        #[cfg(target_os = "ios")]
        let builder = builder.manage(crate::ios_engine::IosEngine::new());
        builder
            .invoke_handler(tauri::generate_handler![
                commands::connection,
                crate::pages::page_put,
                crate::pages::page_drop,
                // Phase 2 iOS native OAuth browser sheet (`SFSafariViewController` / `ASWebAuthenticationSession`).
                #[cfg(target_os = "ios")]
                crate::ios_auth::open_auth_browser,
                #[cfg(target_os = "ios")]
                crate::ios_auth::close_auth_browser,
                // 205 §9: the web view tile's native layer, same contract as the desktop's.
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_view_attach,
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_view_bounds,
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_view_visible,
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_view_navigate,
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_view_reload,
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_view_history,
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_view_close,
                // 209 §4.5: the tile store's cookies and a tile's localStorage.
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_profile_read,
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_profile_write,
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_profile_clear,
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_storage_capture,
                // 209 §4.7: the agent's ops on a tile.
                #[cfg(target_os = "ios")]
                crate::ios_webview::web_view_call,
                // 218 §7: the whiteboard's snapshot of a rect of the main webview.
                #[cfg(target_os = "ios")]
                crate::apple_tile::wb_capture,
                #[cfg(target_os = "ios")]
                crate::ios_keyboard::hardware_keyboard_connected,
                #[cfg(not(target_os = "ios"))]
                crate::wb_capture
            ])
            .setup(|app| {
                #[cfg(target_os = "ios")]
                crate::ios_engine::init(app.handle());
                let builder =
                    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                        .background_color(crate::page_background(system_dark()));
                // WKWebView paints a native "‹ › Done" bar above the keyboard for every
                // focused input (and paints it light under a dark page), and on iPad a
                // shortcut bar / floating-keyboard pill with form chevrons; a chat composer
                // has no use for either. The builder covers the web view, `ios_keyboard`
                // the WKContentView that is actually first responder.
                #[cfg(target_os = "ios")]
                let builder = builder.with_input_accessory_view_builder(|_| None);
                let window = builder.build()?;
                #[cfg(target_os = "ios")]
                window.with_webview(|webview| crate::ios_keyboard::strip(webview.inner()))?;
                #[cfg(target_os = "ios")]
                crate::ios_keyboard::watch_hardware_keyboard(app.handle());
                #[cfg(not(target_os = "ios"))]
                let _ = window;
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
            RunEvent::WindowEvent {
                event: WindowEvent::Suspended | WindowEvent::Focused(false),
                ..
            } => background::hold(),
            RunEvent::WindowEvent {
                event: WindowEvent::Resumed | WindowEvent::Focused(true),
                ..
            } => background::release(),
            RunEvent::Exit => {
                crate::ios_keyboard::unwatch_hardware_keyboard();
                crate::ios_engine::shutdown(app);
                crate::apple_cookies::unwatch(app);
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
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};

    use anyhow::Context;
    #[cfg(target_os = "macos")]
    use tauri::menu::Submenu;
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
    use tauri::{AppHandle, Builder, Emitter, Manager, WebviewWindow, WindowEvent, Wry};

    use crate::{commands, companion, island, launch, models, point, quit, sso, update};

    /// 166 R1: argv of the login item's launch (all desktops write it; only macOS reads it, `dock::launch`).
    pub(crate) const AUTOSTART_ARG: &str = "--autostart";

    /// Centre the window on the first show of a fresh profile (window-state restores it afterwards).
    #[derive(Default)]
    struct Flags {
        position_on_show: AtomicBool,
        /// 210 §7.4 item 2: live public shares of the mesh, as the webview's `store/public-shares` counts them.
        sharing: AtomicU32,
    }

    /// Tray menu id of "Stop public sharing (N)"; the shell calls the engine itself ([`crate::share::stop_all`]),
    /// so it works with the main window hidden.
    const SHARE_STOP_ALL: &str = "share_stop_all";

    /// 210 §7.4 item 2 (G4): the webview reports how many shares are live; the tray menu gains or loses
    /// "Stop public sharing (N)". The tray title is the webview's (`App.tsx` badge), so it stays one writer.
    pub(crate) fn set_sharing(app: &AppHandle, live: u32) -> tauri::Result<()> {
        if app.state::<Flags>().sharing.swap(live, SeqCst) == live {
            return Ok(());
        }
        match app.tray_by_id("main") {
            Some(tray) => tray.set_menu(Some(tray_menu(app)?)),
            None => Ok(()),
        }
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
                Some(vec![AUTOSTART_ARG]),
            ))
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
            .manage(quit::Quit::default())
            .manage(Flags::default())
            .manage(sso::Sso::default())
            .manage(crate::webview::Views::default())
            // 205 §5: Tauri's `asset` handler, minus `web-*` webviews (`asset.rs`).
            .register_asynchronous_uri_scheme_protocol(crate::asset::SCHEME, crate::asset::handler);
        // 58 §6.2: one island command set on every desktop OS; the `NSPanel` plugin is macOS's.
        #[cfg(target_os = "macos")]
        let builder = builder.plugin(tauri_nspanel::init());
        // 134 D10: on Windows a `theuth://` click starts a second `theuth-desktop.exe` with the URL in
        // argv (W1S); single-instance goes first and, with its `deep-link` feature, hands argv to the
        // deep-link plugin before the callback. The NSIS installer writes `HKCU\Software\Classes\theuth`.
        #[cfg(target_os = "windows")]
        let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show(app)
        }));
        // 130 D10: `theuth://` — Linux has no companion to send them and no single-instance (§5.9).
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let builder = builder.plugin(tauri_plugin_deep_link::init());
        let builder = builder.invoke_handler(tauri::generate_handler![
            commands::connection,
            commands::launch_log,
            commands::relaunch,
            commands::startup_status,
            commands::quit_ack,
            commands::share_tray,
            commands::share_stop_all,
            commands::open_window,
            commands::legal_text,
            commands::pasteboard_change_count,
            commands::pasteboard_clear_if,
            sso::sso_listen,
            sso::sso_wait,
            sso::sso_cancel,
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
            crate::pages::page_put,
            crate::pages::page_drop,
            companion::companion_alive,
            companion::companion_login_item_status,
            crate::webview::web_view_attach,
            crate::webview::web_view_bounds,
            crate::webview::web_view_visible,
            crate::webview::web_view_navigate,
            crate::webview::web_view_reload,
            crate::webview::web_view_history,
            crate::webview::web_view_close,
            crate::webview::web_profile_read,
            crate::webview::web_profile_write,
            crate::webview::web_profile_clear,
            crate::webview::web_storage_capture,
            crate::webview::web_view_call,
            // 218 §7: the whiteboard's snapshot of a rect of the main webview.
            #[cfg(target_os = "macos")]
            crate::apple_tile::wb_capture,
            #[cfg(not(target_os = "macos"))]
            crate::wb_capture,
        ]);
        // The native menu bar is macOS's (⌘C/⌘V reach the webview through it); GTK would paint it inside the window.
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
                WindowEvent::Destroyed => {
                    crate::webview::on_window_destroyed(window);
                    if window.label().starts_with(commands::POPOUT_PREFIX) {
                        let _ = window.app_handle().emit("popout-closed", window.label());
                    }
                }
                _ => {}
            })
            .setup(|app| {
                // 166 D1: Dock icon and ⌘Tab entry; the tray / companion menu bar icon stays as it is.
                #[cfg(target_os = "macos")]
                app.set_activation_policy(tauri::ActivationPolicy::Regular);
                setup(app.handle())?;
                island::init(app.handle())?;
                point::init(app.handle())?;
                companion::init(app.handle())?;
                #[cfg(target_os = "macos")]
                crate::dock::init(app.handle());
                // 221 D20 `lock_on_sleep`: sleep and screen lock → `power-lock` → the webview → the engine.
                #[cfg(target_os = "macos")]
                crate::power::init(app.handle());
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                deep_links(app.handle());
                // 166 R1: last, so a user launch shows a window that is fully set up.
                #[cfg(target_os = "macos")]
                crate::dock::launch(app.handle());
                Ok(())
            })
    }

    /// Asset scope (120 §9d: only the roots the UI loads as images — never `token`, the vault or
    /// the databases), tray menu + click handler on the tray the config created; first-show flag.
    fn setup(app: &AppHandle) -> anyhow::Result<()> {
        // `allow_directory` registers glob patterns and never stats: a root the engine creates later
        // (`artifacts/` on the first `show`) is covered, a root that never appears matches nothing.
        let data_dir = launch::data_dir();
        for root in ["attachments", "browser", "computer", "artifacts", "spill"] {
            app.asset_protocol_scope()
                .allow_directory(data_dir.join(root), true)
                .with_context(|| format!("allow {root}/ in asset protocol"))?;
        }
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
        tray.set_menu(Some(tray_menu(app)?))?;
        tray.on_tray_icon_event(|tray, event| {
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

    /// The tray's menu; 171: also restored when the pre-quit sheet cancels a quit (`quit.rs`).
    pub(crate) fn tray_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
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
        // 210 §7.4 item 2: first in the menu while anything is public; no confirmation (Stop is never gated).
        let live = app.state::<Flags>().sharing.load(SeqCst);
        if live > 0 {
            menu.prepend(&PredefinedMenuItem::separator(app)?)?;
            menu.prepend(&MenuItem::with_id(
                app,
                SHARE_STOP_ALL,
                format!("Stop public sharing ({live})"),
                true,
                None::<&str>,
            )?)?;
        }
        Ok(menu)
    }

    /// Tauri's default macOS menu minus About, with Quit as our own item so ⌘Q goes
    /// through the `quit-requested` handshake instead of `terminate:` (25 §4.5:
    /// the menu stays so ⌘C/⌘V/⌘A/⌘W reach the webview).
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
                        &MenuItem::with_id(
                            app,
                            PASTE_PLAIN,
                            "Paste as Plain Text",
                            true,
                            Some(PASTE_PLAIN_ACCEL),
                        )?,
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

    /// 165 D5: the Edit-menu item that pastes without the clipboard's HTML (rich paste keeps links).
    #[cfg(target_os = "macos")]
    const PASTE_PLAIN: &str = "paste_plain";
    /// ⇧⌘V: no UI keymap, menu or global shortcut binds it. Tauri drops an accelerator it cannot parse
    /// silently, hence the test.
    #[cfg(target_os = "macos")]
    const PASTE_PLAIN_ACCEL: &str = "Shift+CmdOrCtrl+V";

    /// `-[WKWebView pasteAsPlainText:]` on the focused window: WebKit fires a `paste` whose `clipboardData`
    /// holds only `text/plain`, which the composer pastes as typed text (`ui/src/lib/paste-markdown.ts`).
    #[cfg(target_os = "macos")]
    fn paste_plain(app: &AppHandle) {
        let Some(win) = app
            .webview_windows()
            .into_values()
            .find(|w| w.is_focused().unwrap_or(false))
        else {
            return;
        };
        let sent = win.with_webview(|wv| {
            use objc2::runtime::AnyObject;
            let view = wv.inner().cast::<AnyObject>();
            // SAFETY: `inner()` is the window's live WKWebView and `with_webview` runs on the main thread;
            // `pasteAsPlainText:` takes an `id` sender (nil) and returns void. The responds check keeps a
            // WebKit without the selector from raising.
            unsafe {
                if (*view).class().responds_to(objc2::sel!(pasteAsPlainText:)) {
                    let () =
                        objc2::msg_send![view, pasteAsPlainText: std::ptr::null_mut::<AnyObject>()];
                }
            }
        });
        log_err(sent, "paste as plain text");
    }

    /// Shared by the tray menu and the app menu (same ids).
    fn on_menu(app: &AppHandle, id: &str) {
        match id {
            "open" => show(app),
            "new" => navigate(app, "/"),
            "knowledge" => navigate(app, "/knowledge"),
            "meeting" => navigate(app, "/meetings/new"),
            "settings" => navigate(app, "/settings/general"),
            "quit" => quit::request(app),
            SHARE_STOP_ALL => crate::share::stop_all(app),
            #[cfg(target_os = "macos")]
            PASTE_PLAIN => paste_plain(app),
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

    pub(crate) fn show(app: &AppHandle) {
        let Some(win) = main_window(app) else {
            return;
        };
        if app.state::<Flags>().position_on_show.swap(false, SeqCst) {
            log_err(win.center(), "centre window");
        }
        log_err(win.show(), "show window");
        log_err(win.set_focus(), "focus window");
    }

    /// Show the window and tell the router where to go.
    fn navigate(app: &AppHandle, route: &str) {
        show(app);
        log_err(app.emit("navigate", route), "emit navigate");
    }

    /// 130 D10: `theuth://attention/<id>` · `theuth://c/<session>` · `theuth://new` · `theuth://meetings` (149) from the companion
    /// (or anything else) → [`navigate`]; `theuth://quit` → [`quit::request`]; unknown shapes are logged and dropped. On Windows a cold start
    /// by a link has the URL in argv: the plugin parsed it in its own setup, before this listener existed,
    /// so `get_current()` replays it (macOS delivers `RunEvent::Opened` later and reads `None` here).
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn deep_links(app: &AppHandle) {
        use tauri_plugin_deep_link::DeepLinkExt;
        let handle = app.clone();
        let open = move |urls: Vec<tauri::Url>| {
            for url in urls {
                if is_quit_link(&url) {
                    tracing::info!("deep link: quit requested ({url})");
                    quit::request(&handle);
                    continue;
                }
                match deep_link_route(&url) {
                    Some(route) => navigate(&handle, &route),
                    None => tracing::info!("deep link ignored: {url}"),
                }
            }
        };
        if let Ok(Some(urls)) = app.deep_link().get_current() {
            open(urls);
        }
        app.deep_link().on_open_url(move |event| open(event.urls()));
    }

    /// `theuth://quit`: the companion's tray › Quit theuth — the whole app, one quit (130 W7) → [`quit::request`].
    /// Both companions open this link (163: the Swift one aims it at the running shell; on Windows the
    /// single-instance plugin hands it over). A plain `terminate()` would skip the handshake and its overlay.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    fn is_quit_link(url: &tauri::Url) -> bool {
        url.scheme() == "theuth"
            && url.host_str() == Some("quit")
            && url.path().trim_matches('/').is_empty()
    }

    /// The app route for a `theuth://` URL, or `None` for anything the companion does not send.
    /// 171 §14.3: `theuth://c/<session>?cell=<name|ordinal>` keeps the `cell` query (a slug or an
    /// ordinal, `[a-z0-9-]{1,48}`); every other query, and a `cell` value outside that set, is dropped.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    fn deep_link_route(url: &tauri::Url) -> Option<String> {
        if url.scheme() != "theuth" {
            return None;
        }
        let rest = url.path().trim_matches('/');
        match (url.host_str()?, rest.is_empty()) {
            ("attention", false) => Some(format!("/attention/{rest}")),
            ("c", false) => Some(match url.query_pairs().find(|(k, _)| k == "cell") {
                Some((_, cell))
                    if (1..=48).contains(&cell.len())
                        && cell
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') =>
                {
                    format!("/c/{rest}?cell={cell}")
                }
                _ => format!("/c/{rest}"),
            }),
            ("new", true) => Some("/new".into()),
            // 149 §5.2: the companion's idle island pill opens the meetings list.
            ("meetings", true) => Some("/meetings".into()),
            _ => None,
        }
    }

    fn log_err<T, E: std::fmt::Display>(r: Result<T, E>, what: &str) {
        if let Err(e) = r {
            tracing::warn!(error = %e, "{what}");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{deep_link_route, is_quit_link};

        #[cfg(target_os = "macos")]
        #[test]
        fn paste_plain_accelerator_parses() {
            // Same grammar as muda's menu accelerators; an unparsable one would vanish without an error.
            assert!(
                super::PASTE_PLAIN_ACCEL
                    .parse::<tauri_plugin_global_shortcut::Shortcut>()
                    .is_ok()
            );
        }

        #[test]
        fn deep_link_routes() {
            let route = |s: &str| deep_link_route(&tauri::Url::parse(s).unwrap());
            assert_eq!(
                route("theuth://attention/ask:abc:1").as_deref(),
                Some("/attention/ask:abc:1")
            );
            assert_eq!(route("theuth://c/s-42/").as_deref(), Some("/c/s-42"));
            assert_eq!(route("theuth://new").as_deref(), Some("/new"));
            assert_eq!(route("theuth://new/extra"), None);
            assert_eq!(route("theuth://meetings").as_deref(), Some("/meetings"));
            assert_eq!(route("theuth://meetings/").as_deref(), Some("/meetings"));
            assert_eq!(route("theuth://meetings/abc"), None);
            assert_eq!(route("theuth://attention"), None);
            assert_eq!(route("theuth://settings/general"), None);
            assert_eq!(route("https://attention/x"), None);
        }

        #[test]
        fn deep_link_keeps_the_cell_query() {
            let route = |s: &str| deep_link_route(&tauri::Url::parse(s).unwrap());
            assert_eq!(
                route("theuth://c/s-42?cell=preview-ir-prod1").as_deref(),
                Some("/c/s-42?cell=preview-ir-prod1")
            );
            assert_eq!(
                route("theuth://c/s-42/?cell=2").as_deref(),
                Some("/c/s-42?cell=2")
            );
            assert_eq!(
                route("theuth://c/s-42?x=1&cell=apply").as_deref(),
                Some("/c/s-42?cell=apply")
            );
            assert_eq!(route("theuth://c/s-42?x=1").as_deref(), Some("/c/s-42"));
            assert_eq!(
                route("theuth://c/s-42?cell=a%26b%3D1").as_deref(),
                Some("/c/s-42")
            );
            assert_eq!(route("theuth://c/s-42?cell=").as_deref(), Some("/c/s-42"));
            assert_eq!(route("theuth://c?cell=apply"), None);
            assert_eq!(
                route("theuth://attention/ask:abc:1?cell=x").as_deref(),
                Some("/attention/ask:abc:1")
            );
            assert_eq!(route("theuth://new?cell=x").as_deref(), Some("/new"));
        }

        #[test]
        fn quit_link_is_not_a_route() {
            let url = |s: &str| tauri::Url::parse(s).unwrap();
            assert!(is_quit_link(&url("theuth://quit")));
            assert!(is_quit_link(&url("theuth://quit/")));
            assert!(!is_quit_link(&url("theuth://quit/now")));
            assert!(!is_quit_link(&url("theuth://new")));
            assert!(!is_quit_link(&url("https://quit")));
            assert_eq!(deep_link_route(&url("theuth://quit")), None);
        }
    }
}
