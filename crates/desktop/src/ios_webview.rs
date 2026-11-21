//! The web view tile's native layer in the iOS shell (205 §9): one raw `WKWebView` per `web-<16 hex>` label, a
//! sibling of Tauri's own `WKWebView` placed directly above it, framed over the placeholder the UI measures. Same
//! seven commands, arguments, errors and events as the desktop's `webview.rs` (205 §4.3); Tauri's
//! `Window::add_child` is desktop-only and wry's `build_as_child` is unsupported on iOS, so the view is built
//! here through UIKit.
//!
//! Isolation (205 §5): each view has its own `WKWebViewConfiguration` with the persistent data store
//! [`DATA_STORE`] (iOS 17+), and nothing else: no script message handler, no URL scheme handler, no user
//! script. It is not a Tauri webview, so it gets none of Tauri's schemes, IPC or plugin init scripts: the
//! desktop's IPC fence, `asset` handler check and links script have no counterpart here.
//!
//! Navigation (205 §4.3, §5): the main frame loads only what [`allowed`] passes; subframes also
//! `about:blank` / `about:srcdoc`, `data:` and `blob:` ([`navigation_allowed`]). A navigation without a
//! target frame (`target=_blank`, `window.open`) is cancelled and opened in the system browser, at most once
//! per [`may_open`] interval per view.
//!
//! Every UIKit / WebKit call runs on the main thread: commands hop there through the caller's
//! `Webview::with_webview`, which also hands over Tauri's `WKWebView` for the coordinate conversion. The views
//! and their delegates (WebKit holds delegates weakly) live in a main-thread-only registry.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::time::Instant;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, extern_class, extern_methods,
    msg_send,
};
use objc2_foundation::{
    NSDictionary, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSURL, NSURLRequest, NSUUID,
};
use objc2_ui_kit::{UIApplication, UIResponder, UIScrollView, UIView};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKUIDelegate,
    WKWebViewConfiguration, WKWebsiteDataStore,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Url, Webview};

use crate::web_rules::{
    CLOSED_EVENT, Closed, DATA_STORE, STATE_EVENT, allowed, app_url, check_label, check_url,
    may_open, navigation_allowed, no_view,
};

extern_class!(
    /// `WKWebView` as UIKit declares it; `objc2-web-kit` 0.3 binds the class for macOS only (wry declares
    /// its own copy for the same reason). Only the members this module uses.
    #[unsafe(super(UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    struct WKWebView;
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
    );
}

/// Key paths observed on every view: a title or URL that changes without a navigation callback
/// (`document.title = …`, `history.pushState`).
const OBSERVED: [&str; 2] = ["title", "URL"];

/// What a view's delegate needs to report and police it.
struct DelegateIvars {
    label: String,
    app: AppHandle,
    /// [`app_url`] at creation.
    fence: Option<Url>,
    /// The last system-browser open ([`may_open`]).
    last_open: Cell<Option<Instant>>,
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
            _view: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let policy = if self.permits(action) {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            };
            handler.call((policy,));
        }

        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn did_start(&self, view: &WKWebView, _navigation: Option<&AnyObject>) {
            self.publish(view, true);
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        fn did_commit(&self, view: &WKWebView, _navigation: Option<&AnyObject>) {
            self.publish(view, true);
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, view: &WKWebView, _navigation: Option<&AnyObject>) {
            self.publish(view, false);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, view: &WKWebView, _navigation: Option<&AnyObject>, _error: &AnyObject) {
            self.publish(view, false);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(
            &self,
            view: &WKWebView,
            _navigation: Option<&AnyObject>,
            _error: &AnyObject,
        ) {
            self.publish(view, false);
        }
    }

    unsafe impl WKUIDelegate for Delegate {
        /// `window.open` (and a `target=_blank` the policy check let through): the system browser, never a view.
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
    }

    /// NSKeyValueObserving: [`OBSERVED`] changed.
    impl Delegate {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            _key_path: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            if let Some(view) = object.and_then(|o| o.downcast_ref::<WKWebView>()) {
                self.publish(view, view.is_loading());
            }
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, ivars: DelegateIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: `NSObject`'s `init` on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }

    /// The navigation policy (205 §4.3, §5); a navigation with no target frame goes to the browser instead.
    fn permits(&self, action: &WKNavigationAction) -> bool {
        let Some(url) = action_url(action) else {
            return false;
        };
        let fence = self.ivars().fence.as_ref();
        // SAFETY: plain property reads of a live navigation action.
        match unsafe { action.targetFrame() } {
            None => {
                self.open_external(&url);
                false
            }
            Some(frame) if unsafe { frame.isMainFrame() } => allowed(&url, fence),
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

    /// Emit `web-view://state` with the view's current URL and title.
    fn publish(&self, view: &WKWebView, loading: bool) {
        let state = State {
            label: self.ivars().label.clone(),
            url: view
                .url()
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string())
                .unwrap_or_default(),
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

/// Run `f` on the main thread with Tauri's `WKWebView` of the calling webview, and wait for its result.
async fn on_main<T: Send + 'static>(
    caller: &Webview,
    f: impl FnOnce(MainThreadMarker, &WKWebView) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    caller
        .with_webview(move |platform| {
            let mtm = MainThreadMarker::new().expect("with_webview runs on the main thread");
            // SAFETY: on iOS `inner()` is the webview's `WKWebView`, alive for the duration of the call.
            let main = unsafe { &*platform.inner().cast::<WKWebView>() };
            let _ = tx.send(f(mtm, main));
        })
        .map_err(|e| e.to_string())?;
    // The reply comes from the main thread's next turn; wait for it off the async workers.
    tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|_| "the main thread dropped the web view call".to_owned())?
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
    // SAFETY: plain configuration setters on a fresh object; the identifier API exists from iOS 17 and the
    // app's minimum is 17.4.
    let config = unsafe {
        let config = WKWebViewConfiguration::new(mtm);
        let store =
            WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(DATA_STORE), mtm);
        config.setWebsiteDataStore(&store);
        config
    };
    let view = WKWebView::init_with_frame(WKWebView::alloc(mtm), frame, &config);
    view.setClipsToBounds(true);
    let delegate = Delegate::new(
        mtm,
        DelegateIvars {
            label: label.to_owned(),
            fence: app_url(&app),
            app,
            last_open: Cell::new(None),
        },
    );
    view.set_navigation_delegate(Some(ProtocolObject::from_ref(&*delegate)));
    view.set_ui_delegate(Some(ProtocolObject::from_ref(&*delegate)));
    for key in OBSERVED {
        // SAFETY: removed in `destroy` before the delegate or the view is released.
        unsafe {
            view.addObserver_forKeyPath_options_context(
                &delegate,
                &NSString::from_str(key),
                NSKeyValueObservingOptions::New,
                null_mut(),
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
        // SAFETY: added in `create` for exactly these key paths.
        unsafe { view.removeObserver_forKeyPath(&delegate, &NSString::from_str(key)) };
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
        VIEWS.with_borrow_mut(|views| views.insert(label, entry));
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
    if delta != -1 && delta != 1 {
        return Err(format!("history delta must be -1 or 1, got {delta}"));
    }
    on_main(&webview, move |_, _| {
        with_view(&label, |e| {
            if delta < 0 {
                e.view.go_back();
            } else {
                e.view.go_forward();
            }
        })
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
