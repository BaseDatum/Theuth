//! Native iOS OAuth browser sheet keeping the app foregrounded so loopback callbacks
//! (`localhost:1455` OpenAI, `localhost:54545` Anthropic, ephemeral MCP loopback ports)
//! complete without iOS suspending the embedded engine.
//!
//! - Standard login (`ephemeral: None | Some(false)`): presents `SFSafariViewController`
//!   modally on the topmost `UIViewController`.
//! - Ephemeral / multi-account login (`ephemeral: Some(true)`): starts an
//!   `ASWebAuthenticationSession` with `prefersEphemeralWebBrowserSession = YES` and a
//!   no-op callback scheme (`"theuth-noop"`), anchored to the active key `UIWindow`.
//!
//! When the user dismisses or cancels the sheet, this module clears its stored reference
//! and emits `"auth-browser-dismissed"` to the webview. When the UI closes the sheet
//! programmatically via [`close_auth_browser`] (e.g. after `LoginFinished`), the stored
//! reference is removed first so `"auth-browser-dismissed"` is NOT emitted.

#![cfg(target_os = "ios")]

use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::time::Duration;

use block2::{DynBlock, RcBlock};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyProtocol, NSObject};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, extern_class,
    extern_methods, msg_send,
};
use objc2_foundation::{NSError, NSObjectProtocol, NSString, NSURL};
use objc2_ui_kit::{
    UIApplication, UIResponder, UISceneActivationState, UIViewController, UIWindow, UIWindowScene,
};
use tauri::{AppHandle, Emitter, Url};
use tokio::sync::oneshot;

#[link(name = "SafariServices", kind = "framework")]
unsafe extern "C" {}

#[link(name = "AuthenticationServices", kind = "framework")]
unsafe extern "C" {}

/// Event emitted to the webview when the user cancels/dismisses the native OAuth browser sheet.
const DISMISSED_EVENT: &str = "auth-browser-dismissed";
/// No-op callback scheme for ephemeral `ASWebAuthenticationSession` (real callbacks hit loopback).
const NOOP_CALLBACK_SCHEME: &str = "theuth-noop";
/// Bound on waiting for the UIKit main thread to process an open/close command.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(5);

extern_class!(
    /// `SFSafariViewController` from `SafariServices.framework`.
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    struct SFSafariViewController;
);

impl SFSafariViewController {
    extern_methods!(
        #[unsafe(method(initWithURL:))]
        fn init_with_url(this: Allocated<Self>, url: &NSURL) -> Retained<Self>;

        #[unsafe(method(setDelegate:))]
        fn set_delegate(&self, delegate: Option<&NSObject>);
    );
}

extern_class!(
    /// `ASWebAuthenticationSession` from `AuthenticationServices.framework`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct ASWebAuthenticationSession;
);

impl ASWebAuthenticationSession {
    extern_methods!(
        #[unsafe(method(initWithURL:callbackURLScheme:completionHandler:))]
        unsafe fn init_with_url(
            this: Allocated<Self>,
            url: &NSURL,
            callback_url_scheme: Option<&NSString>,
            completion_handler: &DynBlock<dyn Fn(*mut NSURL, *mut NSError)>,
        ) -> Retained<Self>;

        #[unsafe(method(setPrefersEphemeralWebBrowserSession:))]
        fn set_prefers_ephemeral(&self, ephemeral: bool);

        #[unsafe(method(setPresentationContextProvider:))]
        fn set_presentation_context_provider(&self, provider: Option<&NSObject>);

        #[unsafe(method(start))]
        fn start(&self) -> bool;

        #[unsafe(method(cancel))]
        fn cancel(&self);
    );
}

struct SafariDelegateIvars {
    app: AppHandle,
    session_id: u64,
}

define_class!(
    /// `SFSafariViewControllerDelegate` receiving `safariViewControllerDidFinish:`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SafariDelegateIvars]
    struct SafariDelegate;

    unsafe impl NSObjectProtocol for SafariDelegate {}

    impl SafariDelegate {
        #[unsafe(method(safariViewControllerDidFinish:))]
        fn safari_view_controller_did_finish(&self, _controller: &SFSafariViewController) {
            on_browser_finished(&self.ivars().app, self.ivars().session_id);
        }
    }
);

impl SafariDelegate {
    fn new(mtm: MainThreadMarker, app: AppHandle, session_id: u64) -> Retained<Self> {
        attach_protocol(Self::class(), c"SFSafariViewControllerDelegate");
        let this = Self::alloc(mtm).set_ivars(SafariDelegateIvars { app, session_id });
        // SAFETY: `NSObject`'s `init` on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}

struct AuthContextProviderIvars {
    window: Retained<UIWindow>,
}

define_class!(
    /// `ASWebAuthenticationPresentationContextProviding` returning the active key `UIWindow`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = AuthContextProviderIvars]
    struct AuthContextProvider;

    unsafe impl NSObjectProtocol for AuthContextProvider {}

    impl AuthContextProvider {
        #[unsafe(method_id(presentationAnchorForWebAuthenticationSession:))]
        fn presentation_anchor(
            &self,
            _session: &ASWebAuthenticationSession,
        ) -> Retained<UIWindow> {
            self.ivars().window.clone()
        }
    }
);

impl AuthContextProvider {
    fn new(mtm: MainThreadMarker, window: Retained<UIWindow>) -> Retained<Self> {
        attach_protocol(
            Self::class(),
            c"ASWebAuthenticationPresentationContextProviding",
        );
        let this = Self::alloc(mtm).set_ivars(AuthContextProviderIvars { window });
        // SAFETY: `NSObject`'s `init` on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}

enum BrowserKind {
    Safari {
        controller: Retained<SFSafariViewController>,
        _delegate: Retained<SafariDelegate>,
    },
    AuthSession {
        session: Retained<ASWebAuthenticationSession>,
        _provider: Retained<AuthContextProvider>,
    },
}

struct ActiveBrowser {
    id: u64,
    kind: BrowserKind,
}

thread_local! {
    /// Currently presented auth browser sheet/session; main thread only.
    static ACTIVE: RefCell<Option<ActiveBrowser>> = const { RefCell::new(None) };
    /// Monotonic id distinguishing each presentation from stale dismiss/cancel callbacks.
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

/// Open `url` in an in-app iOS OAuth browser sheet (`SFSafariViewController` by default, or
/// an ephemeral `ASWebAuthenticationSession` when `ephemeral == Some(true)`).
#[tauri::command]
pub async fn open_auth_browser(
    app: AppHandle,
    url: String,
    ephemeral: Option<bool>,
) -> Result<(), String> {
    let parsed = Url::parse(&url).map_err(|e| format!("invalid auth url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("unsupported auth url scheme: {}", parsed.scheme()));
    }
    let ephemeral = ephemeral.unwrap_or(false);
    let url_str = parsed.to_string();
    let app_inner = app.clone();
    on_main(&app, move |mtm| {
        present_auth_browser(mtm, app_inner, &url_str, ephemeral)
    })
    .await
}

/// Programmatically close the active OAuth browser sheet without emitting `"auth-browser-dismissed"`.
#[tauri::command]
pub async fn close_auth_browser(app: AppHandle) -> Result<(), String> {
    on_main(&app, |_| {
        dismiss_active(true);
        Ok(())
    })
    .await
}

async fn on_main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce(MainThreadMarker) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = oneshot::channel();
    app.run_on_main_thread(move || {
        let res = match MainThreadMarker::new() {
            Some(mtm) => f(mtm),
            None => Err("run_on_main_thread executed off the main thread".into()),
        };
        let _ = tx.send(res);
    })
    .map_err(|e| e.to_string())?;
    tokio::time::timeout(MAIN_THREAD_TIMEOUT, rx)
        .await
        .map_err(|_| "timed out waiting for iOS main thread".to_owned())?
        .map_err(|_| "iOS main thread task dropped before replying".to_owned())?
}

fn present_auth_browser(
    mtm: MainThreadMarker,
    app: AppHandle,
    url: &str,
    ephemeral: bool,
) -> Result<(), String> {
    let ns_url = NSURL::URLWithString(&NSString::from_str(url))
        .ok_or_else(|| format!("invalid NSURL `{url}`"))?;
    let window = key_window(mtm).ok_or_else(|| "no active iOS window".to_owned())?;

    // Replace any existing sheet silently without emitting `auth-browser-dismissed`.
    dismiss_active(false);
    let session_id = next_session_id();

    if ephemeral {
        let provider = AuthContextProvider::new(mtm, window);
        let app_cb = app.clone();
        let completion = RcBlock::new(move |_callback_url: *mut NSURL, _error: *mut NSError| {
            let app_main = app_cb.clone();
            let _ = app_cb.run_on_main_thread(move || {
                on_browser_finished(&app_main, session_id);
            });
        });
        let scheme = NSString::from_str(NOOP_CALLBACK_SCHEME);
        // SAFETY: valid `NSURL`, valid `NSString`, and a retained `RcBlock` matching
        // `ASWebAuthenticationSessionCompletionHandler`.
        let session = unsafe {
            ASWebAuthenticationSession::init_with_url(
                ASWebAuthenticationSession::alloc(mtm),
                &ns_url,
                Some(&scheme),
                &completion,
            )
        };
        session.set_prefers_ephemeral(true);
        let provider_obj: &NSObject = &provider;
        session.set_presentation_context_provider(Some(provider_obj));
        if !session.start() {
            session.set_presentation_context_provider(None);
            return Err("ASWebAuthenticationSession failed to start".into());
        }
        ACTIVE.replace(Some(ActiveBrowser {
            id: session_id,
            kind: BrowserKind::AuthSession {
                session,
                _provider: provider,
            },
        }));
        Ok(())
    } else {
        let top = topmost_view_controller(&window)
            .ok_or_else(|| "no UIViewController available to present auth browser".to_owned())?;
        let delegate = SafariDelegate::new(mtm, app, session_id);
        let controller =
            SFSafariViewController::init_with_url(SFSafariViewController::alloc(mtm), &ns_url);
        let delegate_obj: &NSObject = &delegate;
        controller.set_delegate(Some(delegate_obj));
        top.presentViewController_animated_completion(&controller, true, None);
        ACTIVE.replace(Some(ActiveBrowser {
            id: session_id,
            kind: BrowserKind::Safari {
                controller,
                _delegate: delegate,
            },
        }));
        Ok(())
    }
}

/// Dismiss/cancel any active auth browser without emitting `"auth-browser-dismissed"`.
fn dismiss_active(animated: bool) {
    let Some(active) = ACTIVE.take() else {
        return;
    };
    match active.kind {
        BrowserKind::Safari {
            controller,
            _delegate,
        } => {
            controller.set_delegate(None);
            controller.dismissViewControllerAnimated_completion(animated, None);
        }
        BrowserKind::AuthSession { session, _provider } => {
            session.cancel();
            session.set_presentation_context_provider(None);
        }
    }
}

/// Called on the main thread when the user finishes/cancels the active browser sheet.
/// Ignores callbacks for sheets already closed programmatically via [`close_auth_browser`].
fn on_browser_finished(app: &AppHandle, session_id: u64) {
    let matched = ACTIVE.with_borrow_mut(|slot| {
        if slot.as_ref().is_some_and(|active| active.id == session_id) {
            *slot = None;
            true
        } else {
            false
        }
    });
    if matched && let Err(e) = app.emit(DISMISSED_EVENT, ()) {
        tracing::warn!(error = %e, "emit {DISMISSED_EVENT}");
    }
}

fn next_session_id() -> u64 {
    NEXT_ID.with(|cell| {
        let id = cell.get();
        cell.set(id.wrapping_add(1));
        id
    })
}

fn attach_protocol(cls: &AnyClass, name: &CStr) {
    if let Some(proto) = AnyProtocol::get(name) {
        // SAFETY: `class_addProtocol` is idempotent and `cls` is our own registered class.
        unsafe {
            objc2::ffi::class_addProtocol((cls as *const AnyClass).cast_mut(), proto);
        }
    }
}

/// Locate the active key `UIWindow` across connected `UIWindowScene`s.
fn key_window(mtm: MainThreadMarker) -> Option<Retained<UIWindow>> {
    let app = UIApplication::sharedApplication(mtm);
    let mut fallback: Option<Retained<UIWindow>> = None;
    for scene in app.connectedScenes() {
        let Ok(window_scene) = scene.downcast::<UIWindowScene>() else {
            continue;
        };
        let active = window_scene.activationState() == UISceneActivationState::ForegroundActive;
        if let Some(key) = window_scene.keyWindow() {
            if active {
                return Some(key);
            }
            if fallback.is_none() {
                fallback = Some(key);
            }
        }
        for window in window_scene.windows() {
            if window.isKeyWindow() {
                if active {
                    return Some(window);
                }
                if fallback.is_none() {
                    fallback = Some(window);
                }
            } else if fallback.is_none() {
                fallback = Some(window);
            }
        }
    }
    fallback
}

/// Walk the presented-controller chain of `window` to find the topmost non-dismissing VC.
fn topmost_view_controller(window: &UIWindow) -> Option<Retained<UIViewController>> {
    let mut top = window.rootViewController()?;
    while let Some(next) = top.presentedViewController() {
        if next.isBeingDismissed() {
            break;
        }
        top = next;
    }
    Some(top)
}
