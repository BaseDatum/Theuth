//! No keyboard bar over the composer (31 §“iOS keyboard”). WebKit decorates the keyboard of every
//! focused editable from its private `WKContentView` (the first responder, not the `WKWebView`):
//!
//! - `inputAccessoryView`: the iPhone "‹ › Done" strip. wry's `with_input_accessory_view_builder`
//!   answers it on the `WKWebView` subclass, which WebKit consults only while it detects the
//!   override; returning nil on the content view as well does not depend on that detection.
//! - `inputAssistantItem`: the iPad shortcut bar. WebKit fills its leading/trailing bar-button
//!   groups with undo/redo/paste and the form-navigation chevrons, which a nil accessory view
//!   leaves alone. With a hardware keyboard attached, iPadOS 26 draws them as a floating pill at
//!   the bottom of the screen (shortcuts/keyboard, language, ⌃ ⌄, dictation) once you type.
//!
//! [`strip`] makes the content view's class answer nil for the first and hands out its assistant
//! item with both groups pinned empty: the item is moved to a subclass whose group setters store
//! `[]`, so WebKit re-populating it on each focus has no effect. The bar-button groups are the
//! only state the subclass touches; the item keeps UIKit's own class underneath.
//!
//! Located by class-name prefix (`WKContentView`, or a KVO subclass of it) under the web view's
//! scroll view, so a renamed or re-parented class is a no-op rather than a crash.

use std::cell::RefCell;
use std::ffi::{CStr, c_void};
use std::mem::transmute;
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, ProtocolObject, Sel};
use objc2::{class, ffi, msg_send, sel};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol,
};
use objc2_ui_kit::UIApplicationDidBecomeActiveNotification;
use tauri::{AppHandle, Emitter};

#[link(name = "GameController", kind = "framework")]
unsafe extern "C" {
    static GCKeyboardDidConnectNotification: &'static NSNotificationName;
    static GCKeyboardDidDisconnectNotification: &'static NSNotificationName;
}

type Observer = (
    Retained<NSNotificationCenter>,
    Retained<ProtocolObject<dyn NSObjectProtocol>>,
);

const UNKNOWN: u8 = 2;
static LAST_CONNECTED: AtomicU8 = AtomicU8::new(UNKNOWN);

thread_local! {
    static OBSERVERS: RefCell<Vec<Observer>> = const { RefCell::new(Vec::new()) };
}

/// Returns whether an external hardware keyboard is currently connected (`GCKeyboard.coalescedKeyboard != nil`).
pub fn is_hardware_keyboard_connected() -> bool {
    let Some(cls) = AnyClass::get(c"GCKeyboard") else {
        return false;
    };
    // SAFETY: `+[GCKeyboard coalescedKeyboard]` is a class getter returning a nullable `GCKeyboard*`.
    let keyboard: *mut AnyObject = unsafe { msg_send![cls, coalescedKeyboard] };
    !keyboard.is_null()
}

/// Tauri IPC query for whether an external hardware keyboard is attached on iOS.
#[tauri::command]
pub fn hardware_keyboard_connected() -> bool {
    is_hardware_keyboard_connected()
}

/// Observe hardware keyboard connect/disconnect and app foregrounding, emitting `hardware-keyboard`
/// whenever the connected state changes. Idempotent if called more than once.
pub fn watch_hardware_keyboard(app: &AppHandle) {
    unwatch_hardware_keyboard();
    LAST_CONNECTED.store(u8::from(is_hardware_keyboard_connected()), Ordering::SeqCst);

    let center = NSNotificationCenter::defaultCenter();
    let emit = |app: AppHandle| {
        move || {
            let connected = is_hardware_keyboard_connected();
            let next = u8::from(connected);
            if LAST_CONNECTED.swap(next, Ordering::SeqCst) != next {
                let _ = app.emit("hardware-keyboard", connected);
            }
        }
    };

    // SAFETY: GameController / UIKit static notification name constants.
    observe(&center, unsafe { GCKeyboardDidConnectNotification }, emit(app.clone()));
    observe(&center, unsafe { GCKeyboardDidDisconnectNotification }, emit(app.clone()));
    observe(
        &center,
        unsafe { UIApplicationDidBecomeActiveNotification },
        emit(app.clone()),
    );
}

/// Remove registered hardware keyboard `NSNotificationCenter` observers (called on `RunEvent::Exit`).
pub fn unwatch_hardware_keyboard() {
    for (center, token) in OBSERVERS.take() {
        // SAFETY: `token` was returned by `addObserverForName:object:queue:usingBlock:` on `center`.
        unsafe { center.removeObserver(token.as_ref()) };
    }
    LAST_CONNECTED.store(UNKNOWN, Ordering::SeqCst);
}

fn observe(
    center: &Retained<NSNotificationCenter>,
    name: &NSNotificationName,
    handler: impl Fn() + 'static,
) {
    let block = block2::RcBlock::new(move |_: NonNull<NSNotification>| handler());
    // SAFETY: no object filter, posting queue; the observer token is kept in `OBSERVERS`
    // until `unwatch_hardware_keyboard` removes it.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
    };
    OBSERVERS.with_borrow_mut(|o| o.push((center.clone(), token)));
}

type Getter = unsafe extern "C-unwind" fn(&AnyObject, Sel) -> *mut AnyObject;
type Setter = unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject);

/// The content view's own `inputAssistantItem`.
static ASSISTANT_ITEM: OnceLock<Getter> = OnceLock::new();
/// (UIKit's assistant-item class, our subclass of it).
static EMPTY_ITEM: OnceLock<(&'static AnyClass, &'static AnyClass)> = OnceLock::new();

/// `webview` is the `WKWebView` (`PlatformWebview::inner`). Main thread only.
pub fn strip(webview: *mut c_void) {
    // SAFETY: tauri hands out the live `WKWebView` pointer for the duration of `with_webview`.
    let Some(webview) = (unsafe { webview.cast::<AnyObject>().as_ref() }) else {
        return;
    };
    let Some(content) = content_view(webview) else {
        tracing::warn!("ios keyboard: no WKContentView under the web view; keyboard bar stays");
        return;
    };
    let cls = content.class();
    // SAFETY: both replacements match the selectors' `@@:` encoding (object return, self, _cmd).
    // `class_replaceMethod` adds the method to `cls` when only a superclass implements it.
    unsafe {
        let cls_mut = (cls as *const AnyClass).cast_mut();
        ffi::class_replaceMethod(
            cls_mut,
            sel!(inputAccessoryView),
            transmute::<Getter, Imp>(no_accessory),
            c"@@:".as_ptr(),
        );
        if ASSISTANT_ITEM.get().is_none() {
            let Some(original) = cls.instance_method(sel!(inputAssistantItem)) else {
                return;
            };
            let original = original.implementation();
            let previous = ffi::class_replaceMethod(
                cls_mut,
                sel!(inputAssistantItem),
                transmute::<Getter, Imp>(empty_assistant_item),
                c"@@:".as_ptr(),
            );
            // `previous` is `cls`'s own implementation when it had one, else nil (inherited).
            let original = previous.unwrap_or(original);
            let _ = ASSISTANT_ITEM.set(transmute::<Imp, Getter>(original));
        }
        // Pin the item now in case WebKit filled it before the first focus asks for it.
        let _: *mut AnyObject = msg_send![content, inputAssistantItem];
        // A first responder already showing the keyboard picks the change up on reload.
        let _: () = msg_send![content, reloadInputViews];
    }
}

/// The `WKContentView` under `webview.scrollView`, searched breadth-first a few levels down.
fn content_view(webview: &AnyObject) -> Option<&AnyObject> {
    // SAFETY: `scrollView` / `subviews` are UIKit getters returning objects owned by the hierarchy.
    let scroll: *mut AnyObject = unsafe { msg_send![webview, scrollView] };
    let mut level: Vec<&AnyObject> = unsafe { scroll.as_ref() }.into_iter().collect();
    for _ in 0..4 {
        let mut next = Vec::new();
        for view in level {
            let subviews: *mut AnyObject = unsafe { msg_send![view, subviews] };
            let Some(subviews) = (unsafe { subviews.as_ref() }) else {
                continue;
            };
            let count: usize = unsafe { msg_send![subviews, count] };
            for i in 0..count {
                let child: *mut AnyObject = unsafe { msg_send![subviews, objectAtIndex: i] };
                let Some(child) = (unsafe { child.as_ref() }) else {
                    continue;
                };
                if is_content_view(child.class().name()) {
                    return Some(child);
                }
                next.push(child);
            }
        }
        level = next;
    }
    None
}

fn is_content_view(name: &CStr) -> bool {
    let name = name.to_bytes();
    name.starts_with(b"WKContent") || name.starts_with(b"NSKVONotifying_WKContent")
}

unsafe extern "C-unwind" fn no_accessory(_: &AnyObject, _: Sel) -> *mut AnyObject {
    std::ptr::null_mut()
}

unsafe extern "C-unwind" fn empty_assistant_item(this: &AnyObject, cmd: Sel) -> *mut AnyObject {
    let original = ASSISTANT_ITEM.get().expect("installed with the method");
    // SAFETY: the content view's own implementation, called with its own receiver and selector.
    let item = unsafe { original(this, cmd) };
    if let Some(item) = unsafe { item.as_ref() } {
        pin_empty(item);
    }
    item
}

/// Move `item` onto the empty-groups subclass of its class and clear both groups.
fn pin_empty(item: &AnyObject) {
    let cls = item.class();
    let pair = *EMPTY_ITEM.get_or_init(|| (cls, empty_item_class(cls)));
    let (uikit, ours) = pair;
    if cls != ours {
        if cls != uikit {
            // An unexpected class (another subclass, KVO): clear it once, leave its class alone.
            clear(item, cls);
            return;
        }
        // SAFETY: `ours` is a direct subclass of `uikit` adding methods only (no ivars), so the
        // object layout is unchanged.
        unsafe { ffi::object_setClass((item as *const AnyObject).cast_mut(), ours) };
    }
    clear(item, uikit);
}

/// Set both groups to `[]` through `uikit`'s setters (bypassing ours, which discard the argument).
fn clear(item: &AnyObject, uikit: &AnyClass) {
    // SAFETY: `+[NSArray array]` is an empty immutable array; the setters take an `NSArray`.
    unsafe {
        let empty: Retained<AnyObject> = msg_send![class!(NSArray), array];
        let empty = Retained::as_ptr(&empty).cast_mut();
        let _: () = msg_send![super(item, uikit), setLeadingBarButtonGroups: empty];
        let _: () = msg_send![super(item, uikit), setTrailingBarButtonGroups: empty];
    }
}

/// `TheuthEmptyAssistantItem`: `uikit` with `setLeadingBarButtonGroups:` and
/// `setTrailingBarButtonGroups:` storing `[]` whatever they are given.
fn empty_item_class(uikit: &'static AnyClass) -> &'static AnyClass {
    const NAME: &CStr = c"TheuthEmptyAssistantItem";
    if let Some(existing) = AnyClass::get(NAME) {
        return existing;
    }
    unsafe extern "C-unwind" fn set_empty(this: &AnyObject, _: Sel, _: *mut AnyObject) {
        if let Some((uikit, _)) = EMPTY_ITEM.get() {
            clear(this, uikit);
        }
    }
    // SAFETY: a fresh class pair under a unique name, methods added before registration; `set_empty`
    // matches the setters' `v@:@` encoding.
    unsafe {
        let cls = ffi::objc_allocateClassPair(uikit, NAME.as_ptr(), 0);
        let Some(cls_ref) = cls.as_mut() else {
            return uikit;
        };
        for sel in [
            sel!(setLeadingBarButtonGroups:),
            sel!(setTrailingBarButtonGroups:),
        ] {
            ffi::class_addMethod(
                cls_ref,
                sel,
                transmute::<Setter, Imp>(set_empty),
                c"v@:@".as_ptr(),
            );
        }
        ffi::objc_registerClassPair(cls_ref);
        &*cls
    }
}
