//! 166: theuth in the Dock and the ⌘Tab switcher (`ActivationPolicy::Regular`, `lib.rs`). A Dock click
//! (`RunEvent::Reopen`, [`reopen`]) or ⌘Tab (`NSApplicationDidBecomeActiveNotification`, [`init`]) with the
//! main window hidden shows it. Dock › Quit and other user `terminate:`s take the 163 graceful quit through
//! an `applicationShouldTerminate:` added to tao's app delegate, which has none; a logout, restart or
//! shutdown still terminates at once (`RunEvent::Exit` → `quit::on_exit`). R1: a user launch shows the
//! window ([`launch`]); the login item's (`--autostart`) stays hidden.

use std::cell::Cell;
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::{NSApp, NSApplicationDidBecomeActiveNotification, NSApplicationTerminateReply};
use objc2_foundation::{NSNotification, NSNotificationCenter};
use tauri::{AppHandle, Manager};

use crate::{commands, point, quit};

/// tao activates the app once in `applicationDidFinishLaunching:`; that activation is not a ⌘Tab. One that
/// comes later than this after [`init`] is a user's even if it is the first (the launch one was refused).
const LAUNCH_ACTIVATION: Duration = Duration::from_secs(5);

/// `kAEQuitReason` (`AERegistry.h`): present on the quit Apple event of a logout, restart or shutdown.
const QUIT_REASON: u32 = u32::from_be_bytes(*b"why?");

/// For [`terminate_reply`], which runs inside an Objective-C callback with no handle of its own.
static APP: OnceLock<AppHandle> = OnceLock::new();

/// What a visible window is to the ⌘Tab rule (D3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Main,
    Popout,
    /// The island and the `overlay-<n>` panels: non-activating, ordered in for the life of the process (alpha 0
    /// while concealed), so always "visible" to AppKit.
    Panel,
    /// A label nobody listed here: a normal window until said otherwise, so it never pops the main window.
    Other,
}

fn kind(label: &str) -> Kind {
    match label {
        "main" => Kind::Main,
        "island" => Kind::Panel,
        l if l.starts_with(point::OVERLAY_PREFIX) => Kind::Panel,
        l if l.starts_with(commands::POPOUT_PREFIX) => Kind::Popout,
        _ => Kind::Other,
    }
}

/// D3: an activation shows the main window only after launch, outside a quit, and when no theuth window but
/// the panels is on screen (a visible popout or main comes forward by itself).
fn show_on_activate(
    launch_done: bool,
    quitting: bool,
    visible: impl IntoIterator<Item = Kind>,
) -> bool {
    launch_done && !quitting && visible.into_iter().all(|k| k == Kind::Panel)
}

/// D4: a system session end (any `kAEQuitReason`) terminates now — cancelling would abort the logout. Any
/// other `terminate:` (Dock › Quit, AppleScript `quit`, the companion's fallback) is cancelled and becomes
/// [`quit::request`], whose worker exits through `NSApp.stop`, not `terminate:`.
fn terminate_reply(quit_reason: Option<u32>) -> NSApplicationTerminateReply {
    match quit_reason {
        Some(_) => NSApplicationTerminateReply::TerminateNow,
        None => NSApplicationTerminateReply::TerminateCancel,
    }
}

/// Setup, main thread: the ⌘Tab observer and the delegate's `applicationShouldTerminate:`.
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
    watch_activation(app);
    add_should_terminate();
}

/// D2: Dock click. `has_visible_windows` is no use — the island and overlays are always ordered in.
pub fn reopen(app: &AppHandle) {
    let main_visible = app
        .get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    if !main_visible && !quit::requested(app) {
        crate::desktop::show(app);
    }
}

/// R1: a user launch (Dock, Finder, Spotlight, the companion's "Open theuth") shows the window; the login
/// item's launch (`--autostart`) stays hidden. Idempotent with the launch activation: [`LAUNCH_ACTIVATION`]
/// skips it, and `show` twice is one show.
pub fn launch(app: &AppHandle) {
    rewrite_login_item(app);
    if show_at_launch(std::env::args().skip(1)) {
        crate::desktop::show(app);
    }
}

fn show_at_launch(args: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    !args
        .into_iter()
        .any(|a| a.as_ref() == crate::desktop::AUTOSTART_ARG)
}

/// A LaunchAgent written before R1 has no `--autostart`, so its login would show the window: rewrite it once
/// (the plist is `~/Library/LaunchAgents/<product>.plist`, auto-launch 0.6 `macos.rs` `get_file`). Only from
/// an `.app`, so a dev build never repoints the login item at `target/`.
fn rewrite_login_item(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let autolaunch = app.autolaunch();
    if !autolaunch.is_enabled().unwrap_or(false) {
        return;
    }
    let plist = dirs::home_dir()
        .map(|h| {
            h.join("Library/LaunchAgents")
                .join(format!("{}.plist", app.package_info().name))
        })
        .and_then(|p| std::fs::read_to_string(p).ok());
    let bundled =
        std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"));
    if plist.as_deref().is_some_and(plist_has_autostart_arg) || !bundled {
        return;
    }
    match autolaunch.enable() {
        Ok(()) => tracing::info!(
            "login item rewritten with {}",
            crate::desktop::AUTOSTART_ARG
        ),
        Err(e) => tracing::warn!(error = %e, "rewrite login item"),
    }
}

fn plist_has_autostart_arg(plist: &str) -> bool {
    plist.contains(&format!(
        "<string>{}</string>",
        crate::desktop::AUTOSTART_ARG
    ))
}

fn watch_activation(app: &AppHandle) {
    let app = app.clone();
    let started = Instant::now();
    let seen = Cell::new(false);
    let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| {
        let launch_done = seen.replace(true) || started.elapsed() > LAUNCH_ACTIVATION;
        let visible = app
            .webview_windows()
            .into_iter()
            .filter(|(_, w)| w.is_visible().unwrap_or(false))
            .map(|(label, _)| kind(&label));
        if show_on_activate(launch_done, quit::requested(&app), visible) {
            crate::desktop::show(&app);
        }
    });
    // SAFETY: the name is AppKit's static; no object filter, no queue (the block runs on the posting thread —
    // NSApp posts this on the main thread); the centre holds the observer for the life of the process.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidBecomeActiveNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(token);
}

/// tao 0.37's `TaoAppDelegateParent` implements no `applicationShouldTerminate:`, so NSApp terminated at once;
/// AppKit asks `respondsToSelector:` on every `terminate:`, so adding the method after launch takes effect.
fn add_should_terminate() {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!(
            "dock: applicationShouldTerminate: off the main thread; Dock › Quit stays abrupt"
        );
        return;
    };
    // SAFETY: `-[NSApplication delegate]` returns the (retained by NSApp) delegate or nil.
    let delegate: Option<Retained<AnyObject>> = unsafe { msg_send![&*NSApp(mtm), delegate] };
    let Some(delegate) = delegate else {
        tracing::warn!("dock: no app delegate; Dock › Quit stays abrupt");
        return;
    };
    if !install_should_terminate(delegate.class()) {
        tracing::warn!(
            "dock: the app delegate already answers applicationShouldTerminate:; left as is"
        );
    }
}

/// `class_addMethod`; `false` when `cls` already has the selector.
fn install_should_terminate(cls: &AnyClass) -> bool {
    let cls: *const AnyClass = cls;
    // SAFETY: `should_terminate` matches `Q@:@` — NSUInteger return (`NSApplicationTerminateReply` is
    // `repr(transparent)` over it), self, _cmd, the `id` sender — and unwinds into AppKit only by panic.
    unsafe {
        let imp = std::mem::transmute::<
            extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> NSApplicationTerminateReply,
            Imp,
        >(should_terminate);
        objc2::ffi::class_addMethod(
            cls.cast_mut(),
            sel!(applicationShouldTerminate:),
            imp,
            c"Q@:@".as_ptr(),
        )
    }
    .as_bool()
}

extern "C-unwind" fn should_terminate(
    _: &AnyObject,
    _: Sel,
    _: *mut AnyObject,
) -> NSApplicationTerminateReply {
    let reason = quit_reason();
    let reply = terminate_reply(reason);
    tracing::info!(
        reason = ?reason.map(|r| String::from_utf8_lossy(&r.to_be_bytes()).into_owned()),
        now = reply == NSApplicationTerminateReply::TerminateNow,
        "applicationShouldTerminate"
    );
    // `APP` is set before the method exists (`init`).
    if reply == NSApplicationTerminateReply::TerminateCancel
        && let Some(app) = APP.get()
    {
        quit::request(app);
    }
    reply
}

/// `kAEQuitReason` of the Apple event being handled, if `terminate:` came from one that carries it.
fn quit_reason() -> Option<u32> {
    // SAFETY: `+[NSAppleEventManager sharedAppleEventManager]` returns the singleton; `currentAppleEvent` and
    // `attributeDescriptorForKeyword:` (an `AEKeyword` = `u32`) return a descriptor or nil; `enumCodeValue`
    // returns an `OSType` = `u32`.
    unsafe {
        let manager: Option<Retained<AnyObject>> = msg_send![
            AnyClass::get(c"NSAppleEventManager")?,
            sharedAppleEventManager
        ];
        let event: Option<Retained<AnyObject>> = msg_send![&*manager?, currentAppleEvent];
        let reason: Option<Retained<AnyObject>> =
            msg_send![&*event?, attributeDescriptorForKeyword: QUIT_REASON];
        let code: u32 = msg_send![&*reason?, enumCodeValue];
        Some(code)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Kind, install_should_terminate, kind, plist_has_autostart_arg, show_at_launch,
        show_on_activate, terminate_reply,
    };
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ClassBuilder, NSObject};
    use objc2::{ClassType, msg_send};
    use objc2_app_kit::NSApplicationTerminateReply;

    /// The callback through the real Objective-C dispatch, its registered `Q@:@` checked against the Rust
    /// signature: outside an Apple event there is no quit reason, so the reply is Cancel (the graceful quit;
    /// no `APP` in a test, so nothing is requested).
    #[test]
    fn should_terminate_answers_through_objc() {
        let cls = ClassBuilder::new(c"Dock166Probe", NSObject::class())
            .expect("fresh class")
            .register();
        assert!(install_should_terminate(cls));
        assert!(!install_should_terminate(cls));
        cls.verify_sel::<(*mut AnyObject,), NSApplicationTerminateReply>(objc2::sel!(
            applicationShouldTerminate:
        ))
        .expect("encoding matches the Rust signature");
        // SAFETY: `+new` on an NSObject subclass; the method was just added with that signature.
        let reply: NSApplicationTerminateReply = unsafe {
            let obj: Retained<AnyObject> = msg_send![cls, new];
            msg_send![&*obj, applicationShouldTerminate: std::ptr::null_mut::<AnyObject>()]
        };
        assert_eq!(reply, NSApplicationTerminateReply::TerminateCancel);
    }

    #[test]
    fn only_the_login_item_launch_stays_hidden() {
        assert!(show_at_launch(Vec::<String>::new()));
        // Finder / Dock may add a process serial number; anything but our flag is a user launch.
        assert!(show_at_launch(["-psn_0_12345"]));
        assert!(!show_at_launch(["--autostart"]));
        assert!(!show_at_launch(["-psn_0_1", "--autostart"]));
        assert!(show_at_launch(["--autostart=no", "autostart"]));
    }

    #[test]
    fn old_login_items_are_told_apart() {
        // auto-launch 0.6 `build_launch_agent_plist`: one `<string>` per ProgramArguments entry.
        let exe = "<string>/Applications/theuth.app/Contents/MacOS/theuth-desktop</string>";
        let old = format!("<key>ProgramArguments</key>\n  <array>{exe}</array>");
        let new = format!(
            "<key>ProgramArguments</key>\n  <array>{exe}<string>--autostart</string></array>"
        );
        assert!(!plist_has_autostart_arg(&old));
        assert!(plist_has_autostart_arg(&new));
    }

    #[test]
    fn window_kinds() {
        assert_eq!(kind("main"), Kind::Main);
        assert_eq!(kind("island"), Kind::Panel);
        assert_eq!(kind("overlay-0"), Kind::Panel);
        assert_eq!(kind("overlay-3"), Kind::Panel);
        assert_eq!(kind("popout-note-abc"), Kind::Popout);
        assert_eq!(kind("settings"), Kind::Other);
    }

    #[test]
    fn activation_shows_main_only_when_nothing_but_panels_is_up() {
        use Kind::*;
        // ⌘Tab with main hidden: the always-up panels do not count.
        assert!(show_on_activate(true, false, []));
        assert!(show_on_activate(true, false, [Panel, Panel]));
        // The launch activation and one during a quit never show it.
        assert!(!show_on_activate(false, false, []));
        assert!(!show_on_activate(true, true, [Panel]));
        // Any normal theuth window already on screen comes forward by itself.
        assert!(!show_on_activate(true, false, [Main]));
        assert!(!show_on_activate(true, false, [Panel, Popout]));
        assert!(!show_on_activate(true, false, [Other]));
    }

    #[test]
    fn a_session_end_is_never_cancelled() {
        let code = |s: &[u8; 4]| Some(u32::from_be_bytes(*s));
        for reason in [b"logo", b"rlgo", b"rrst", b"rest", b"shut", b"rsdn"] {
            assert_eq!(
                terminate_reply(code(reason)),
                NSApplicationTerminateReply::TerminateNow
            );
        }
        assert_eq!(
            terminate_reply(None),
            NSApplicationTerminateReply::TerminateCancel
        );
    }
}
