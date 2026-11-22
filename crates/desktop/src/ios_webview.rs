//! The web view tile's native layer in the iOS shell (205 §9): one raw `WKWebView` per `web-<16 hex>` label, a
//! sibling of Tauri's own `WKWebView` placed directly above it, framed over the placeholder the UI measures. Same
//! seven commands, arguments, errors and events as the desktop's `webview.rs` (205 §4.3); Tauri's
//! `Window::add_child` is desktop-only and wry's `build_as_child` is unsupported on iOS, so the view is built
//! here through UIKit.
//!
//! Isolation (205 §5): each view has its own `WKWebViewConfiguration` with the persistent data store
//! [`DATA_STORE`](crate::web_rules::DATA_STORE) (iOS 17+; before that the default store, as wry does), and
//! nothing else: no script message handler, no URL scheme handler, no user
//! script. It is not a Tauri webview, so it gets none of Tauri's schemes, IPC or plugin init scripts: the
//! desktop's IPC fence, `asset` handler check and links script have no counterpart here.
//!
//! Navigation (205 §4.3, §5): the main frame loads only what [`allowed`] passes; subframes also
//! `about:blank` / `about:srcdoc`, `data:` and `blob:` ([`navigation_allowed`]). A new-window request
//! (`target=_blank`, `window.open`) reaches `createWebView…`, which opens it in the system browser, at most once
//! per [`may_open`] interval per view, and never creates a view; so does a main-frame response WebKit cannot
//! show (a download). JavaScript panels (`alert`, `confirm`, `prompt`) are native alerts.
//!
//! Every UIKit / WebKit call runs on the main thread: commands hop there through the caller's
//! `Webview::with_webview`, which also hands over Tauri's `WKWebView` for the coordinate conversion. The views
//! and their delegates (WebKit holds delegates weakly) live in a main-thread-only registry.
//!
//! `objc2-web-kit` 0.3 declares the delegate protocols' methods for macOS only, so the compiler does not check
//! the selectors or signatures below against the protocols on iOS; [`create`] asserts in debug builds that
//! every one of [`DELEGATE_SELECTORS`] is registered.
//!
//! The browser profile (209 §4.5): `web_profile_read`, `web_profile_write`, `web_profile_clear` and
//! `web_storage_capture` read, write and empty the [`DATA_STORE`](crate::web_rules::DATA_STORE) store and
//! read or write a view's localStorage through `apple_cookies`, which [`create`] also starts watching for
//! `web-profile://changed`. They answer the main webview only, and `PROFILE_UNSUPPORTED` before iOS 17.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::{NonNull, null_mut};
use std::rc::Rc;
use std::sync::mpsc::sync_channel;
use std::time::Instant;

use block2::{DynBlock, RcBlock};
use objc2::rc::{Allocated, Retained, Weak};
use objc2::runtime::{AnyObject, Bool, NSObject, ProtocolObject, Sel};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, extern_class, extern_methods,
    msg_send, sel,
};
use objc2_foundation::{
    NSDictionary, NSError, NSHTTPURLResponse, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSURL, NSURLRequest,
};
use objc2_ui_kit::{
    UIAlertAction, UIAlertActionStyle, UIAlertController, UIAlertControllerStyle, UIApplication,
    UIResponder, UIScrollView, UITextField, UIView,
};
use objc2_web_kit::{
    WKContentWorld, WKFrameInfo, WKNavigationAction, WKNavigationActionPolicy,
    WKNavigationDelegate, WKNavigationResponse, WKNavigationResponsePolicy, WKUIDelegate,
    WKWebViewConfiguration, WKWebsiteDataStore,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Url, Webview};

use crate::apple_cookies::{
    MAIN_TIMEOUT, apply_storage, available, capture, captured, clear, data_store, read, wait,
    watch, write, written,
};
use crate::web_rules::profile::{
    Cookie, CookieKey, Storage, Written, check_caller, plan_write, web_origin,
};
use crate::web_rules::{
    CLOSED_EVENT, Closed, STATE_EVENT, allowed, app_url, check_label, check_url, may_open,
    navigation_allowed, no_view,
};

extern_class!(
    /// `WKWebView` as UIKit declares it; `objc2-web-kit` 0.3 binds the class for macOS only (wry declares
    /// its own copy for the same reason). Only the members this module and `apple_cookies` use.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    pub(crate) struct WKWebView;
);

impl WKWebView {
    extern_methods!(
        #[unsafe(method(initWithFrame:configuration:))]
        fn init_with_frame(
            this: Allocated<Self>,
            frame: NSRect,
            configuration: &WKWebViewConfiguration,
        ) -> Retained<Self>;

        #[unsafe(method(setNavigationDelegate:))]
        fn set_navigation_delegate(
            &self,
            delegate: Option<&ProtocolObject<dyn WKNavigationDelegate>>,
        );

        #[unsafe(method(setUIDelegate:))]
        fn set_ui_delegate(&self, delegate: Option<&ProtocolObject<dyn WKUIDelegate>>);

        #[unsafe(method(loadRequest:))]
        fn load_request(&self, request: &NSURLRequest) -> Option<Retained<AnyObject>>;

        #[unsafe(method(title))]
        fn title(&self) -> Option<Retained<NSString>>;

        #[unsafe(method(URL))]
        fn url(&self) -> Option<Retained<NSURL>>;

        #[unsafe(method(isLoading))]
        fn is_loading(&self) -> bool;

        #[unsafe(method(goBack))]
        fn go_back(&self) -> Option<Retained<AnyObject>>;

        #[unsafe(method(goForward))]
        fn go_forward(&self) -> Option<Retained<AnyObject>>;

        #[unsafe(method(reload))]
        fn reload(&self) -> Option<Retained<AnyObject>>;

        #[unsafe(method(stopLoading))]
        fn stop_loading(&self);

        #[unsafe(method(scrollView))]
        fn scroll_view(&self) -> Retained<UIScrollView>;

        /// The selector `objc2-web-kit` binds on macOS (`WKWebView.rs`), with the same name and types, so
        /// `apple_cookies::capture` calls one method on both platforms.
        #[allow(non_snake_case)]
        #[unsafe(method(callAsyncJavaScript:arguments:inFrame:inContentWorld:completionHandler:))]
        pub(crate) unsafe fn callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &self,
            function_body: &NSString,
            arguments: Option<&NSDictionary<NSString, AnyObject>>,
            frame: Option<&WKFrameInfo>,
            content_world: &WKContentWorld,
            completion_handler: Option<&DynBlock<dyn Fn(*mut AnyObject, *mut NSError)>>,
        );
    );
}

/// Key paths observed on every view: a title or URL that changes without a navigation callback
/// (`document.title = …`, `history.pushState`).
const OBSERVED: [&str; 2] = ["title", "URL"];

/// The KVO context of [`Delegate`]'s registrations: its address, unique to this class.
static KVO_CONTEXT: u8 = 0;

fn kvo_context() -> *mut c_void {
    (&raw const KVO_CONTEXT).cast_mut().cast()
}

/// Every delegate method [`Delegate`] implements, checked against the runtime in debug builds.
fn delegate_selectors() -> [Sel; 12] {
    [
        sel!(webView:decidePolicyForNavigationAction:decisionHandler:),
        sel!(webView:decidePolicyForNavigationResponse:decisionHandler:),
        sel!(webView:didStartProvisionalNavigation:),
        sel!(webView:didCommitNavigation:),
        sel!(webView:didFinishNavigation:),
        sel!(webView:didFailNavigation:withError:),
        sel!(webView:didFailProvisionalNavigation:withError:),
        sel!(webViewWebContentProcessDidTerminate:),
        sel!(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:),
        sel!(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:),
        sel!(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:),
        sel!(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:),
    ]
}

/// What a view's delegate needs to report and police it.
struct DelegateIvars {
    label: String,
    app: AppHandle,
    /// [`app_url`] at creation.
    fence: Option<Url>,
    /// The last system-browser open ([`may_open`]).
    last_open: Cell<Option<Instant>>,
    /// The main-frame URL the policy last allowed and that has not committed: what the state reports while
    /// it loads, and after it fails, until the view's own URL changes (205 §9).
    target: RefCell<Option<String>>,
}

define_class!(
    /// Navigation + UI delegate and KVO observer of one web view.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            view: &WKWebView,
            action: &WKNavigationAction,
            handler: &DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let policy = if self.permits(view, action) {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            };
            handler.call((policy,));
        }

        /// A main-frame response WebKit cannot show (or an attachment) would leave the tile blank: the
        /// system browser gets it instead.
        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_response(
            &self,
            view: &WKWebView,
            response: &WKNavigationResponse,
            handler: &DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            let policy = if download(response) {
                // SAFETY: plain property reads of a live navigation response.
                let url = unsafe { response.response() }
                    .URL()
                    .and_then(|u| u.absoluteString())
                    .and_then(|s| Url::parse(&s.to_string()).ok());
                if let Some(url) = url {
                    self.open_external(&url);
                }
                self.ivars().target.replace(None);
                self.publish(view, view.is_loading());
                WKNavigationResponsePolicy::Cancel
            } else {
                WKNavigationResponsePolicy::Allow
            };
            handler.call((policy,));
        }

        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn did_start(&self, view: &WKWebView, _navigation: Option<&AnyObject>) {
            self.publish(view, true);
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        fn did_commit(&self, view: &WKWebView, _navigation: Option<&AnyObject>) {
            self.ivars().target.replace(None);
            self.publish(view, true);
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, view: &WKWebView, _navigation: Option<&AnyObject>) {
            self.ivars().target.replace(None);
            self.publish(view, false);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, view: &WKWebView, _navigation: Option<&AnyObject>, _error: &AnyObject) {
            self.ivars().target.replace(None);
            self.publish(view, false);
        }

        /// Keeps [`DelegateIvars::target`]: the address field shows the address that failed, as on the desktop.
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(
            &self,
            view: &WKWebView,
            _navigation: Option<&AnyObject>,
            _error: &AnyObject,
        ) {
            self.publish(view, false);
        }

        /// iOS killed the page's content process (memory pressure, a crash): reload, or the tile stays blank.
        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn process_terminated(&self, view: &WKWebView) {
            tracing::warn!(
                "web view {}: content process terminated, reloading",
                self.ivars().label
            );
            view.reload();
            self.publish(view, true);
        }
    }

    unsafe impl WKUIDelegate for Delegate {
        /// `window.open` and `target=_blank`: the system browser, never a view. The only place that opens one
        /// for a page's request.
        #[unsafe(method(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            _view: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &AnyObject,
        ) -> *mut AnyObject {
            if let Some(url) = action_url(action) {
                self.open_external(&url);
            }
            null_mut()
        }

        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn alert_panel(
            &self,
            view: &WKWebView,
            message: &NSString,
            frame: &WKFrameInfo,
            handler: &DynBlock<dyn Fn()>,
        ) {
            let handler = handler.copy();
            let reply = Reply::new(move |()| handler.call(()));
            let alert = panel(self.mtm(), frame, message);
            add_action(&alert, "OK", UIAlertActionStyle::Default, move |_| {
                reply.send(())
            });
            present(view, &alert);
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn confirm_panel(
            &self,
            view: &WKWebView,
            message: &NSString,
            frame: &WKFrameInfo,
            handler: &DynBlock<dyn Fn(Bool)>,
        ) {
            let handler = handler.copy();
            let reply = Reply::new(move |ok: bool| handler.call((Bool::new(ok),)));
            let alert = panel(self.mtm(), frame, message);
            let cancel = reply.clone();
            add_action(&alert, "Cancel", UIAlertActionStyle::Cancel, move |_| {
                cancel.send(false)
            });
            add_action(&alert, "OK", UIAlertActionStyle::Default, move |_| {
                reply.send(true)
            });
            present(view, &alert);
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        fn prompt_panel(
            &self,
            view: &WKWebView,
            prompt: &NSString,
            default_text: Option<&NSString>,
            frame: &WKFrameInfo,
            handler: &DynBlock<dyn Fn(*mut NSString)>,
        ) {
            let handler = handler.copy();
            let reply = Reply::new(move |text: Option<Retained<NSString>>| {
                handler.call((text
                    .as_ref()
                    .map_or(null_mut(), |t| Retained::as_ptr(t).cast_mut()),));
            });
            let alert = panel(self.mtm(), frame, prompt);
            let default_text = default_text.map(|t| NSString::from_str(&t.to_string()));
            let configure = RcBlock::new(move |field: NonNull<UITextField>| {
                // SAFETY: UIKit passes the live text field it just added.
                unsafe { field.as_ref() }.setText(default_text.as_deref());
            });
            alert.addTextFieldWithConfigurationHandler(Some(&configure));
            let cancel = reply.clone();
            add_action(&alert, "Cancel", UIAlertActionStyle::Cancel, move |_| {
                cancel.send(None)
            });
            let weak = Weak::from_retained(&alert);
            add_action(&alert, "OK", UIAlertActionStyle::Default, move |_| {
                let text = weak
                    .load()
                    .and_then(|a| a.textFields())
                    .and_then(|fields| fields.firstObject())
                    .and_then(|field| field.text())
                    .unwrap_or_else(NSString::new);
                reply.send(Some(text));
            });
            present(view, &alert);
        }
    }

    /// NSKeyValueObserving: [`OBSERVED`] changed.
    impl Delegate {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            key_path: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            context: *mut c_void,
        ) {
            if context != kvo_context() {
                return;
            }
            let Some(view) = object.and_then(|o| o.downcast_ref::<WKWebView>()) else {
                return;
            };
            let loading = view.is_loading();
            // The page moved its own address (`pushState`, a fragment) outside a load: report that one.
            if !loading && key_path.is_some_and(|k| k.to_string() == "URL") {
                self.ivars().target.replace(None);
            }
            self.publish(view, loading);
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, ivars: DelegateIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: `NSObject`'s `init` on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }

    /// The navigation policy (205 §4.3, §5). A navigation with no target frame is allowed: WebKit then asks
    /// `createWebView…`, which sends it to the browser and creates nothing. An allowed main-frame
    /// navigation publishes its target URL.
    fn permits(&self, view: &WKWebView, action: &WKNavigationAction) -> bool {
        let Some(url) = action_url(action) else {
            return false;
        };
        let fence = self.ivars().fence.as_ref();
        // SAFETY: plain property reads of a live navigation action.
        match unsafe { action.targetFrame() } {
            None => true,
            Some(frame) if unsafe { frame.isMainFrame() } => {
                let ok = allowed(&url, fence);
                if ok {
                    self.ivars().target.replace(Some(url.to_string()));
                    self.publish(view, view.is_loading());
                }
                ok
            }
            Some(_) => navigation_allowed(&url, fence),
        }
    }

    /// http/https only, at most once per [`may_open`] interval.
    fn open_external(&self, url: &Url) {
        if !matches!(url.scheme(), "http" | "https") {
            return;
        }
        let now = Instant::now();
        let last = &self.ivars().last_open;
        if !may_open(last.get(), now) {
            tracing::warn!("web view new-window request throttled: {url}");
            return;
        }
        last.set(Some(now));
        let Some(ns) = NSURL::URLWithString(&NSString::from_str(url.as_str())) else {
            return;
        };
        // SAFETY: an empty options dictionary is a valid `UIApplicationOpenExternalURLOptionsKey` map.
        unsafe {
            UIApplication::sharedApplication(self.mtm()).openURL_options_completionHandler(
                &ns,
                &NSDictionary::new(),
                None,
            );
        }
    }

    /// Emit `web-view://state` with the pending target URL, else the view's, and the view's title.
    fn publish(&self, view: &WKWebView, loading: bool) {
        let url = self.ivars().target.borrow().clone().unwrap_or_else(|| {
            view.url()
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string())
                .unwrap_or_default()
        });
        let state = State {
            label: self.ivars().label.clone(),
            url,
            title: view.title().map(|s| s.to_string()).unwrap_or_default(),
            loading,
        };
        if let Err(e) = self.ivars().app.emit(STATE_EVENT, state) {
            tracing::warn!("emit {STATE_EVENT}: {e}");
        }
    }
}

/// The request URL of a navigation action, if it parses.
fn action_url(action: &WKNavigationAction) -> Option<Url> {
    // SAFETY: plain property read of a live navigation action.
    let request = unsafe { action.request() };
    Url::parse(&request.URL()?.absoluteString()?.to_string()).ok()
}

/// A main-frame response the view cannot display: a MIME type WebKit cannot show, or
/// `Content-Disposition: attachment`.
fn download(response: &WKNavigationResponse) -> bool {
    // SAFETY: plain property reads of a live navigation response.
    let (main, showable, inner) = unsafe {
        (
            response.isForMainFrame(),
            response.canShowMIMEType(),
            response.response(),
        )
    };
    let attachment = || {
        inner
            .downcast_ref::<NSHTTPURLResponse>()
            .and_then(|http| {
                http.valueForHTTPHeaderField(&NSString::from_str("Content-Disposition"))
            })
            .is_some_and(|v| {
                v.to_string()
                    .trim_start()
                    .to_ascii_lowercase()
                    .starts_with("attachment")
            })
    };
    main && (!showable || attachment())
}

/// A JavaScript panel's completion handler, called exactly once: by [`Reply::send`], or with the panel's
/// default answer (`()`, `false`, `nil`) when the last owner drops it unanswered (the alert could not be
/// presented, or went away without an action).
struct Reply<A: Default>(Cell<Option<Box<dyn FnOnce(A)>>>);

impl<A: Default> Reply<A> {
    fn new(call: impl FnOnce(A) + 'static) -> Rc<Self> {
        Rc::new(Self(Cell::new(Some(Box::new(call)))))
    }

    fn send(&self, answer: A) {
        if let Some(call) = self.0.take() {
            call(answer);
        }
    }
}

impl<A: Default> Drop for Reply<A> {
    fn drop(&mut self) {
        self.send(A::default());
    }
}

/// A native alert for a page's panel, titled with the host of the frame that asked.
fn panel(
    mtm: MainThreadMarker,
    frame: &WKFrameInfo,
    message: &NSString,
) -> Retained<UIAlertController> {
    // SAFETY: plain property read of a live frame info.
    let host = unsafe { frame.request() }
        .URL()
        .and_then(|u| u.absoluteString())
        .and_then(|s| Url::parse(&s.to_string()).ok())
        .and_then(|u| u.host_str().map(str::to_owned));
    UIAlertController::alertControllerWithTitle_message_preferredStyle(
        host.map(|h| NSString::from_str(&h)).as_deref(),
        Some(message),
        UIAlertControllerStyle::Alert,
        mtm,
    )
}

fn add_action(
    alert: &UIAlertController,
    title: &str,
    style: UIAlertActionStyle,
    on_tap: impl Fn(NonNull<UIAlertAction>) + 'static,
) {
    let block = RcBlock::new(on_tap);
    alert.addAction(&UIAlertAction::actionWithTitle_style_handler(
        Some(&NSString::from_str(title)),
        style,
        Some(&block),
        alert.mtm(),
    ));
}

/// Present `alert` from the top-most view controller of `view`'s window. When there is none, nothing is
/// presented and the dropped alert answers its panel with the default.
fn present(view: &WKWebView, alert: &UIAlertController) {
    let Some(mut top) = view.window().and_then(|w| w.rootViewController()) else {
        tracing::warn!("web view panel: no view controller to present from");
        return;
    };
    while let Some(next) = top.presentedViewController() {
        if next.isBeingDismissed() {
            break;
        }
        top = next;
    }
    top.presentViewController_animated_completion(alert, true, None);
}

/// `web-view://state` payload (205 §4.3).
#[derive(Clone, Serialize)]
struct State {
    label: String,
    url: String,
    title: String,
    loading: bool,
}

/// A live view and the delegate it holds weakly.
struct Entry {
    view: Retained<WKWebView>,
    delegate: Retained<Delegate>,
}

thread_local! {
    /// Main thread only: UIKit objects never leave it.
    static VIEWS: RefCell<HashMap<String, Entry>> = RefCell::new(HashMap::new());
}

/// Run `f` on the main thread with Tauri's `WKWebView` of the calling webview, and wait for its result, at most
/// [`crate::apple_cookies::MAIN_TIMEOUT`].
async fn on_main<T: Send + 'static>(
    caller: &Webview,
    f: impl FnOnce(MainThreadMarker, &WKWebView) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = sync_channel(1);
    caller
        .with_webview(move |platform| {
            let mtm = MainThreadMarker::new().expect("with_webview runs on the main thread");
            // SAFETY: on iOS `inner()` is the webview's `WKWebView`, alive for the duration of the call.
            let main = unsafe { &*platform.inner().cast::<WKWebView>() };
            let _ = tx.send(f(mtm, main));
        })
        .map_err(|e| e.to_string())?;
    // The reply comes from the main thread's next turn.
    wait(rx).await
}

/// The UI's rectangle (CSS px of the main webview's layout viewport) in `parent`'s coordinates: CSS px scale
/// by the page zoom into the scroll view's content, whose `convertRect` accounts for the content offset
/// and insets (safe area, the iPadOS window-controls strip).
fn frame_in(main: &WKWebView, parent: &UIView, x: f64, y: f64, width: f64, height: f64) -> NSRect {
    let scroll = main.scroll_view();
    let zoom = scroll.zoomScale();
    let rect = NSRect::new(
        NSPoint::new(x * zoom, y * zoom),
        NSSize::new(width * zoom, height * zoom),
    );
    scroll.convertRect_toView(rect, Some(parent))
}

fn parent_of(main: &WKWebView) -> Result<Retained<UIView>, String> {
    main.superview()
        .ok_or_else(|| "the app's webview has no superview".to_owned())
}

fn with_view<T>(label: &str, f: impl FnOnce(&Entry) -> T) -> Result<T, String> {
    VIEWS.with_borrow(|views| views.get(label).map(f).ok_or_else(|| no_view(label)))
}

fn load(view: &WKWebView, url: &Url) -> Result<(), String> {
    let ns = NSURL::URLWithString(&NSString::from_str(url.as_str()))
        .ok_or_else(|| format!("invalid url `{url}`"))?;
    view.load_request(&NSURLRequest::requestWithURL(&ns));
    Ok(())
}

fn create(
    mtm: MainThreadMarker,
    main: &WKWebView,
    label: &str,
    app: AppHandle,
    url: &Url,
    frame: NSRect,
) -> Result<Entry, String> {
    let parent = parent_of(main)?;
    // SAFETY: plain configuration setters on a fresh object. The named store exists from iOS 17
    // (`data_store` checks); before that the tile uses the default store, as wry does, and the profile
    // commands refuse.
    let config = unsafe {
        let config = WKWebViewConfiguration::new(mtm);
        let store = data_store(mtm).unwrap_or_else(|| WKWebsiteDataStore::defaultDataStore(mtm));
        config.setWebsiteDataStore(&store);
        // iOS defaults to NO, which drops `window.open` outside a user gesture without a delegate call;
        // `createWebView…` sends every one to the browser, throttled by `may_open`.
        config
            .preferences()
            .setJavaScriptCanOpenWindowsAutomatically(true);
        config
    };
    let view = WKWebView::init_with_frame(WKWebView::alloc(mtm), frame, &config);
    // 209 §4.5: from the first tile on, store changes reach the UI as `web-profile://changed`.
    watch(mtm, &app);
    view.setClipsToBounds(true);
    let delegate = Delegate::new(
        mtm,
        DelegateIvars {
            label: label.to_owned(),
            fence: app_url(&app),
            app,
            last_open: Cell::new(None),
            target: RefCell::new(None),
        },
    );
    for selector in delegate_selectors() {
        debug_assert!(
            delegate.respondsToSelector(selector),
            "web view delegate lacks {selector}"
        );
    }
    view.set_navigation_delegate(Some(ProtocolObject::from_ref(&*delegate)));
    view.set_ui_delegate(Some(ProtocolObject::from_ref(&*delegate)));
    for key in OBSERVED {
        // SAFETY: removed in `destroy`, with the same context, before the delegate or the view is released.
        unsafe {
            view.addObserver_forKeyPath_options_context(
                &delegate,
                &NSString::from_str(key),
                NSKeyValueObservingOptions::New,
                kvo_context(),
            );
        }
    }
    parent.insertSubview_aboveSubview(&view, main);
    let entry = Entry { view, delegate };
    if let Err(e) = load(&entry.view, url) {
        destroy(entry);
        return Err(e);
    }
    Ok(entry)
}

/// Detach the delegate and the observers, take the view off screen.
fn destroy(entry: Entry) {
    let Entry { view, delegate } = entry;
    for key in OBSERVED {
        // SAFETY: added in `create` for exactly these key paths and this context.
        unsafe {
            view.removeObserver_forKeyPath_context(
                &delegate,
                &NSString::from_str(key),
                kvo_context(),
            )
        };
    }
    view.set_navigation_delegate(None);
    view.set_ui_delegate(None);
    view.stop_loading();
    view.removeFromSuperview();
}

/// Create `label` at the rectangle, or move an existing one there and show it; an existing web view is not
/// navigated (205 §4.3, §9).
#[tauri::command]
#[allow(clippy::too_many_arguments)] // the contract's argument list (205 §4.3)
pub async fn web_view_attach(
    app: AppHandle,
    webview: Webview,
    label: String,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    check_label(&label)?;
    // Checked up front, used only when the view is new: re-attaching never navigates.
    let parsed = check_url(&app, &url);
    on_main(&webview, move |mtm, main| {
        let parent = parent_of(main)?;
        let frame = frame_in(main, &parent, x, y, width, height);
        if let Ok(()) = with_view(&label, |e| {
            e.view.setFrame(frame);
            e.view.setHidden(false);
        }) {
            return Ok(());
        }
        let entry = create(mtm, main, &label, app, &parsed?, frame)?;
        if let Some(old) = VIEWS.with_borrow_mut(|views| views.insert(label, entry)) {
            destroy(old);
        }
        Ok(())
    })
    .await
}

/// Move and size it (205 §4.3).
#[tauri::command]
pub async fn web_view_bounds(
    webview: Webview,
    label: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    check_label(&label)?;
    on_main(&webview, move |_, main| {
        let parent = parent_of(main)?;
        let frame = frame_in(main, &parent, x, y, width, height);
        with_view(&label, |e| e.view.setFrame(frame))
    })
    .await
}

/// Show or hide it without destroying it (205 §4.3).
#[tauri::command]
pub async fn web_view_visible(
    webview: Webview,
    label: String,
    visible: bool,
) -> Result<(), String> {
    check_label(&label)?;
    on_main(&webview, move |_, _| {
        with_view(&label, |e| e.view.setHidden(!visible))
    })
    .await
}

/// Load `url`, http/https only (205 §4.3).
#[tauri::command]
pub async fn web_view_navigate(
    app: AppHandle,
    webview: Webview,
    label: String,
    url: String,
) -> Result<(), String> {
    check_label(&label)?;
    let parsed = check_url(&app, &url);
    on_main(&webview, move |_, _| {
        with_view(&label, |e| load(&e.view, &parsed?))?
    })
    .await
}

/// Reload (205 §4.3).
#[tauri::command]
pub async fn web_view_reload(webview: Webview, label: String) -> Result<(), String> {
    check_label(&label)?;
    on_main(&webview, move |_, _| {
        with_view(&label, |e| {
            e.view.reload();
        })
    })
    .await
}

/// Back (`-1`) or forward (`1`) (205 §4.3).
#[tauri::command]
pub async fn web_view_history(webview: Webview, label: String, delta: i32) -> Result<(), String> {
    check_label(&label)?;
    on_main(&webview, move |_, _| {
        with_view(&label, |e| match delta {
            -1 => {
                e.view.go_back();
                Ok(())
            }
            1 => {
                e.view.go_forward();
                Ok(())
            }
            _ => Err(format!("history delta must be -1 or 1, got {delta}")),
        })?
    })
    .await
}

/// Destroy it and emit `web-view://closed`; an unknown label is `Ok` (205 §4.3).
#[tauri::command]
pub async fn web_view_close(app: AppHandle, webview: Webview, label: String) -> Result<(), String> {
    check_label(&label)?;
    let key = label.clone();
    let existed = on_main(&webview, move |_, _| {
        let entry = VIEWS.with_borrow_mut(|views| views.remove(&key));
        Ok(entry.map(destroy).is_some())
    })
    .await?;
    if existed && let Err(e) = app.emit(CLOSED_EVENT, Closed { label }) {
        tracing::warn!("emit {CLOSED_EVENT}: {e}");
    }
    Ok(())
}

/// Every cookie of the tiles' store, through WebKit on the named store (209 §4.5). Main webview only.
#[tauri::command]
pub async fn web_profile_read(webview: Webview) -> Result<Vec<Cookie>, String> {
    check_caller(webview.label(), true)?;
    read(webview.app_handle().clone()).await
}

/// Empty the tiles' store: every cookie, and localStorage, sessionStorage and IndexedDB (209 §4.5). Main
/// webview only; emits nothing (the UI reads the store again).
#[tauri::command]
pub async fn web_profile_clear(webview: Webview) -> Result<(), String> {
    check_caller(webview.label(), true)?;
    clear(webview.app_handle().clone()).await
}

/// Set `set` and delete the cookies of `remove` in the tiles' store, and apply each localStorage snapshot of
/// `storage` to the open views of its origin (209 §4.5, D5). Counts only; values are never logged. A
/// loaded page keeps the cookies it has until it loads again: no view is reloaded. Main webview only.
#[tauri::command]
pub async fn web_profile_write(
    webview: Webview,
    set: Vec<Cookie>,
    remove: Vec<CookieKey>,
    storage: Option<Vec<Storage>>,
) -> Result<Written, String> {
    check_caller(webview.label(), true)?;
    available()?;
    let deadline = Instant::now() + MAIN_TIMEOUT;
    let plan = plan_write(set, remove, storage.unwrap_or_default());
    let (tx, rx) = sync_channel(1);
    let pending = on_main(&webview, move |mtm, _| {
        write(mtm, plan.cookies, tx);
        let mut pending = Vec::new();
        VIEWS.with_borrow(|views| {
            for entry in views.values() {
                let origin = entry
                    .view
                    .url()
                    .and_then(|u| u.absoluteString())
                    .and_then(|s| web_origin(&s.to_string()));
                for snapshot in plan
                    .storage
                    .iter()
                    .filter(|s| Some(&s.origin) == origin.as_ref())
                {
                    let (tx, rx) = sync_channel(1);
                    apply_storage(mtm, &entry.view, snapshot, tx);
                    pending.push((snapshot.origin.clone(), rx));
                }
            }
        });
        Ok(pending)
    })
    .await?;
    written(rx, pending, deadline).await
}

/// The top-level origin's localStorage of `label`, read in the isolated world (209 §4.5). Main webview
/// only.
#[tauri::command]
pub async fn web_storage_capture(webview: Webview, label: String) -> Result<Storage, String> {
    check_caller(webview.label(), true)?;
    available()?;
    check_label(&label)?;
    let (tx, rx) = sync_channel(1);
    on_main(&webview, move |mtm, _| {
        with_view(&label, |e| capture(mtm, &e.view, tx))
    })
    .await?;
    captured(rx).await
}
