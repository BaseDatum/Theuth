//! iPadOS 26 window controls (111): ask for the `minimal` windowing-control style so the system draws
//! its close/minimise/zoom buttons in a strip above the webview — part of the safe area — instead of
//! floating them over the top-left of the page (tauri#15521).
//!
//! UIKit asks a `UIWindowSceneDelegate` for the style, which needs the scene lifecycle:
//! `UIApplicationSceneManifest` with `UIApplicationSupportsMultipleScenes = true` in `Info.ios.plist`
//! (tao 0.35 enables its scene plumbing on that key alone). tao 0.35's `TaoSceneDelegate` does not
//! answer the question until tao 0.36 (tao#1250), which tauri 2 cannot take (tauri-runtime-wry pins
//! tao ^0.35), and its `application:configurationForConnectingSceneSession:options:` frees the
//! configuration it returns (tao#1244: release builds crash at launch). [`install`] swaps that method
//! for one that returns a live configuration and teaches `TaoSceneDelegate` the style method.
//! Inert without the manifest; UIKit only sends the style query on iPadOS 26+.
//!
//! ponytail: drop this module and the plist key's `true` once tauri ships tao ≥ 0.36.

use std::mem::transmute;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Imp, Sel};
use objc2::{MainThreadMarker, ffi, sel};
use objc2_foundation::NSString;
use objc2_ui_kit::{
    UIApplication, UIScene, UISceneConfiguration, UISceneSession, UISceneWindowingControlStyle,
};

type SceneConfiguration = unsafe extern "C-unwind" fn(
    &AnyObject,
    Sel,
    &AnyObject,
    &UISceneSession,
    &AnyObject,
) -> *mut UISceneConfiguration;

type WindowingStyle =
    unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject) -> *mut UISceneWindowingControlStyle;

/// tao's `application:configurationForConnectingSceneSession:options:`.
static TAO: OnceLock<SceneConfiguration> = OnceLock::new();

/// Call between `tauri::Builder::build` (tao registers `AppDelegate` in `EventLoop::new`) and
/// `App::run` (`UIApplicationMain` instantiates it). No-op without the scene manifest, which is the
/// only case in which tao registers the method we replace.
pub fn install() {
    let Some(delegate) = AnyClass::get(c"AppDelegate") else {
        return;
    };
    let Some(method) =
        delegate.instance_method(sel!(application:configurationForConnectingSceneSession:options:))
    else {
        return;
    };
    // SAFETY: `scene_configuration` has the method's ObjC signature and, like the original, returns
    // an autoreleased configuration; both function types are a plain `IMP` underneath.
    let tao = unsafe {
        method.set_implementation(transmute::<SceneConfiguration, Imp>(scene_configuration))
    };
    let _ = TAO.set(unsafe { transmute::<Imp, SceneConfiguration>(tao) });
}

unsafe extern "C-unwind" fn scene_configuration(
    this: &AnyObject,
    cmd: Sel,
    app: &AnyObject,
    session: &UISceneSession,
    options: &AnyObject,
) -> *mut UISceneConfiguration {
    // tao's version registers `TaoSceneDelegate` on first call and returns a configuration it has
    // already released (tao#1244): the pointer is discarded, the registration is what we are after.
    let _ = unsafe { TAO.get().expect("install() ran")(this, cmd, app, session, options) };
    let delegate = AnyClass::get(c"TaoSceneDelegate");
    if let Some(delegate) = delegate {
        add_windowing_style(delegate);
    }
    // SAFETY: UIKit sends app-delegate messages on the main thread.
    let mtm = unsafe { MainThreadMarker::new_unchecked() };
    let config = UISceneConfiguration::configurationWithName_sessionRole(
        Some(&NSString::from_str("TaoScene")),
        &session.role(),
        mtm,
    );
    // SAFETY: `TaoSceneDelegate` is a registered `UISceneDelegate` class (or absent → UIKit default).
    unsafe { config.setDelegateClass(delegate) };
    Retained::autorelease_ptr(config)
}

/// `preferredWindowingControlStyleForScene:` → `minimal`. Idempotent: `class_addMethod` and
/// `class_addProtocol` are no-ops once the selector / protocol is already there.
fn add_windowing_style(delegate: &AnyClass) {
    unsafe extern "C-unwind" fn minimal(
        _: &AnyObject,
        _: Sel,
        _: &AnyObject,
    ) -> *mut UISceneWindowingControlStyle {
        // Only iPadOS 26+ sends this, so the class exists whenever it runs.
        Retained::autorelease_ptr(UISceneWindowingControlStyle::minimalStyle())
    }
    let cls = (delegate as *const AnyClass).cast_mut();
    // SAFETY: `minimal` matches the `@@:@` encoding (object return, self, _cmd, scene argument).
    unsafe {
        if let Some(proto) = AnyProtocol::get(c"UIWindowSceneDelegate") {
            ffi::class_addProtocol(cls, proto);
        }
        ffi::class_addMethod(
            cls,
            sel!(preferredWindowingControlStyleForScene:),
            transmute::<WindowingStyle, Imp>(minimal),
            c"@@:@".as_ptr(),
        );
    }
}

/// Close a scene the system opened on its own ("New Window" on iPad). The app is one window; the
/// multi-scene declaration exists only because tao 0.35 keys its scene lifecycle on it.
///
/// ponytail: build a `WebviewWindow` for the scene instead once the UI supports a second window.
pub fn dismiss(app: &tauri::AppHandle, scene: &UIScene) {
    struct OnMain(*mut UISceneSession);
    // SAFETY: carried across threads only to be turned back into a `Retained` on the main thread.
    unsafe impl Send for OnMain {}
    impl OnMain {
        fn into_raw(self) -> *mut UISceneSession {
            self.0
        }
    }
    let session = OnMain(Retained::into_raw(scene.session()));
    let app = app.clone();
    // The event arrives from inside `scene:willConnectToSession:`; tauri runs main-thread tasks
    // inline when already on it, so hop through the runtime to land on a later run-loop turn.
    tauri::async_runtime::spawn(async move {
        let _ = app.run_on_main_thread(move || {
            // SAFETY: the pointer came from `Retained::into_raw` above and is consumed once.
            let session =
                unsafe { Retained::from_raw(session.into_raw()) }.expect("retained session");
            let mtm = MainThreadMarker::new().expect("tauri main-thread task");
            UIApplication::sharedApplication(mtm)
                .requestSceneSessionDestruction_options_errorHandler(&session, None, None);
        });
    });
}
