//! Accessibility reader (design doc 96 §0.10, §6.2). macOS: hit-test the
//! system-wide `AXUIElement` under a point and read the element, the app that
//! owns it and that element's window (title, document URL). Every call is
//! checked, nothing panics on a missing attribute. Other OSes: `granted()` is
//! `false` and the readers answer `None` until UIAutomation/AT-SPI readers exist.
//!
//! Points are logical global with the origin at the top-left of the primary
//! display — the space `AXUIElementCopyElementAtPosition` hit-tests in.
//!
//! Chromium (Chrome, Brave, Edge, Arc, Electron) exposes only the window until an
//! assistive client asks for the tree: a window-level hit sets
//! `AXManualAccessibility` on that application once and retries the hit-test.

use std::collections::HashSet;

/// What the tree reports under a point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxElement {
    /// `AXRole` (`AXLink`, `AXStaticText`, …) without the `AX` prefix, lowercased.
    pub role: String,
    /// `AXTitle`, else `AXDescription`.
    pub label: Option<String>,
    /// `AXValue` stringified.
    pub value: Option<String>,
    /// `AXURL` for links.
    pub url: Option<String>,
}

/// An application and one of its windows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxFront {
    /// The owning process.
    pub pid: i32,
    /// `NSRunningApplication.localizedName`.
    pub app: String,
    /// `NSRunningApplication.bundleIdentifier`.
    pub bundle_id: Option<String>,
    /// The window's `AXTitle`.
    pub window_title: Option<String>,
    /// The window's `AXDocument`, else `AXURL`.
    pub document_url: Option<String>,
}

/// One hit-test: the element under the point (`None` when the hit was the
/// window itself — nothing beyond `front`) and the app/window owning it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AxHit {
    /// The element itself, absent when the hit was the window.
    pub element: Option<AxElement>,
    /// The app and window that own it.
    pub front: Option<AxFront>,
}

/// Hit-tester with per-call memory of which apps already had
/// `AXManualAccessibility` switched on; one per `compose()`.
#[derive(Debug, Default)]
pub struct Reader {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    manual: HashSet<i32>,
}

/// Whether `f` is theuth itself: this process, this process's bundle, or the
/// desktop app (`io.theuth.*`, a separate process from the daemon).
#[must_use]
pub fn is_self(f: &AxFront) -> bool {
    f.pid == std::process::id() as i32
        || f.bundle_id
            .as_deref()
            .is_some_and(|b| b.starts_with("io.theuth.") || Some(b) == own_bundle_id().as_deref())
}

/// Cut `s` to at most `chars` characters on a char boundary, marking the cut with `…`.
#[must_use]
pub fn truncate(s: &str, chars: usize) -> String {
    match s.char_indices().nth(chars) {
        Some((i, _)) => format!("{}…", s[..i].trim_end()),
        None => s.to_owned(),
    }
}

/// `accessibility: on|off`, the line's trailing field (96 §6.6).
#[must_use]
pub fn suffix(on: bool) -> &'static str {
    if on {
        "accessibility: on"
    } else {
        "accessibility: off"
    }
}

/// `AXLink` → `link`, `AXStaticText` → `text`, `AXButton` → `button`.
#[must_use]
pub fn role_word(role: &str) -> String {
    match role.strip_prefix("AX").unwrap_or(role) {
        "StaticText" => "text".to_owned(),
        "TextField" | "TextArea" => "field".to_owned(),
        r => r.to_ascii_lowercase(),
    }
}

/// Whether this process may read the tree (`AXIsProcessTrusted`).
#[must_use]
pub fn granted() -> bool {
    cfg!(target_os = "macos") && crate::input::accessibility_granted()
}

#[cfg(target_os = "macos")]
pub use macos::{frontmost, own_bundle_id};

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::{NonNull, null};
    use std::thread::sleep;
    use std::time::Duration;

    use objc2_app_kit::{NSRunningApplication, NSWorkspace};
    use objc2_application_services::{AXError, AXUIElement};
    use objc2_core_foundation::{
        CFArray, CFBoolean, CFNumber, CFRetained, CFString, CFType, CFURL,
    };

    use super::{AxElement, AxFront, AxHit, Reader, role_word};
    use crate::pointer::Pt;

    /// Retries after switching `AXManualAccessibility` on, and the pause between them.
    const MANUAL_RETRIES: usize = 3;
    const MANUAL_PAUSE: Duration = Duration::from_millis(120);
    /// How far `AXParent` is followed looking for the window.
    const PARENT_DEPTH: usize = 32;

    fn attr(el: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
        let mut out: *const CFType = null();
        // SAFETY: `el` is a live element and `out` a valid slot for the copied value.
        let err =
            unsafe { el.copy_attribute_value(&CFString::from_str(name), NonNull::from(&mut out)) };
        if err != AXError::Success {
            return None;
        }
        // SAFETY: `Copy*` returns +1; `from_raw` takes that reference.
        NonNull::new(out.cast_mut()).map(|p| unsafe { CFRetained::from_raw(p) })
    }

    fn text(v: &CFType) -> Option<String> {
        if let Some(s) = v.downcast_ref::<CFString>() {
            return Some(s.to_string());
        }
        if let Some(u) = v.downcast_ref::<CFURL>() {
            return Some(u.string().to_string());
        }
        if let Some(n) = v.downcast_ref::<CFNumber>() {
            return n
                .as_i64()
                .map(|i| i.to_string())
                .or_else(|| n.as_f64().map(|f| f.to_string()));
        }
        if let Some(b) = v.downcast_ref::<CFBoolean>() {
            return Some(b.as_bool().to_string());
        }
        None
    }

    fn string_attr(el: &AXUIElement, name: &str) -> Option<String> {
        attr(el, name)
            .and_then(|v| text(&v))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    fn element_attr(el: &AXUIElement, name: &str) -> Option<CFRetained<AXUIElement>> {
        attr(el, name).and_then(|v| v.downcast::<AXUIElement>().ok())
    }

    fn role(el: &AXUIElement) -> String {
        string_attr(el, "AXRole").unwrap_or_default()
    }

    fn has_children(el: &AXUIElement) -> bool {
        attr(el, "AXChildren")
            .and_then(|v| v.downcast::<CFArray>().ok())
            .is_some_and(|a| a.count() > 0)
    }

    /// The hit carries nothing below the window: the window itself, or an
    /// empty group / web area (Chromium before its tree is built).
    fn window_level(el: &AXUIElement, role: &str) -> bool {
        role == "AXWindow" || (matches!(role, "AXGroup" | "AXWebArea") && !has_children(el))
    }

    fn pid_of(el: &AXUIElement) -> Option<i32> {
        let mut pid: i32 = 0;
        // SAFETY: `pid` is a valid slot for the owning process id.
        (unsafe { el.pid(NonNull::from(&mut pid)) } == AXError::Success && pid > 0).then_some(pid)
    }

    fn hit_test(global: Pt) -> Option<CFRetained<AXUIElement>> {
        // SAFETY: plain constructor; the handle is released with the `CFRetained`.
        let system = unsafe { AXUIElement::new_system_wide() };
        let mut out: *const AXUIElement = null();
        // SAFETY: `out` is a valid slot for the copied element.
        let err =
            unsafe { system.copy_element_at_position(global.x, global.y, NonNull::from(&mut out)) };
        if err != AXError::Success {
            return None;
        }
        // SAFETY: `Copy*` returns +1.
        NonNull::new(out.cast_mut()).map(|p| unsafe { CFRetained::from_raw(p) })
    }

    /// The element's `AXWindow`, else the nearest `AXParent` with role `AXWindow`.
    fn window_of(el: &AXUIElement) -> Option<CFRetained<AXUIElement>> {
        if let Some(w) = element_attr(el, "AXWindow") {
            return Some(w);
        }
        let mut cur = element_attr(el, "AXParent")?;
        for _ in 0..PARENT_DEPTH {
            if role(&cur) == "AXWindow" {
                return Some(cur);
            }
            cur = element_attr(&cur, "AXParent")?;
        }
        None
    }

    fn front_of(el: &AXUIElement, pid: i32) -> AxFront {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        let (name, bundle_id) = app
            .map(|a| {
                (
                    a.localizedName().map(|s| s.to_string()).unwrap_or_default(),
                    a.bundleIdentifier().map(|s| s.to_string()),
                )
            })
            .unwrap_or_default();
        let (window_title, document_url) = match window_of(el) {
            Some(w) => (
                string_attr(&w, "AXTitle"),
                string_attr(&w, "AXDocument").or_else(|| string_attr(&w, "AXURL")),
            ),
            None => (None, None),
        };
        AxFront {
            pid,
            app: name,
            bundle_id,
            window_title,
            document_url,
        }
    }

    fn element_of(el: &AXUIElement, role: &str) -> AxElement {
        AxElement {
            role: role_word(if role.is_empty() { "element" } else { role }),
            label: string_attr(el, "AXTitle").or_else(|| string_attr(el, "AXDescription")),
            value: string_attr(el, "AXValue"),
            url: string_attr(el, "AXURL"),
        }
    }

    impl Reader {
        /// The element under `global` and the app/window owning it.
        #[must_use]
        pub fn hit_at(&mut self, global: Pt) -> Option<AxHit> {
            let mut el = hit_test(global)?;
            let mut r = role(&el);
            let pid = pid_of(&el);
            if let Some(pid) = pid
                && window_level(&el, &r)
                && self.manual.insert(pid)
            {
                // SAFETY: plain constructor over the pid the element reported.
                let app = unsafe { AXUIElement::new_application(pid) };
                // SAFETY: a live app element, a static CFBoolean.
                let err = unsafe {
                    app.set_attribute_value(
                        &CFString::from_str("AXManualAccessibility"),
                        CFBoolean::new(true),
                    )
                };
                tracing::debug!(
                    pid,
                    ?err,
                    "set AXManualAccessibility for a window-level hit"
                );
                for _ in 0..MANUAL_RETRIES {
                    sleep(MANUAL_PAUSE);
                    let Some(again) = hit_test(global) else { break };
                    let again_role = role(&again);
                    let done = !window_level(&again, &again_role);
                    (el, r) = (again, again_role);
                    if done {
                        break;
                    }
                }
            }
            let front = pid.map(|pid| front_of(&el, pid));
            let element = (r != "AXWindow").then(|| element_of(&el, &r));
            Some(AxHit { element, front })
        }
    }

    /// This process's bundle id (`None` for a bare executable such as the daemon).
    #[must_use]
    pub fn own_bundle_id() -> Option<String> {
        NSRunningApplication::currentApplication()
            .bundleIdentifier()
            .map(|s| s.to_string())
    }

    /// The frontmost app via `NSWorkspace`, then its focused window via AX —
    /// the last resort when no hit resolved an app.
    #[must_use]
    pub fn frontmost() -> Option<AxFront> {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let pid = app.processIdentifier();
        // SAFETY: plain constructor over a pid the OS just reported.
        let ax_app = unsafe { AXUIElement::new_application(pid) };
        let window = element_attr(&ax_app, "AXFocusedWindow");
        let (window_title, document_url) = match window {
            Some(w) => (
                string_attr(&w, "AXTitle"),
                string_attr(&w, "AXDocument").or_else(|| string_attr(&w, "AXURL")),
            ),
            None => (None, None),
        };
        Some(AxFront {
            pid,
            app: app
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
            window_title,
            document_url,
        })
    }
}

#[cfg(not(target_os = "macos"))]
impl Reader {
    /// No reader on this OS.
    #[must_use]
    pub fn hit_at(&mut self, _global: crate::pointer::Pt) -> Option<AxHit> {
        None
    }
}

/// No reader on this OS.
#[cfg(not(target_os = "macos"))]
#[must_use]
pub fn frontmost() -> Option<AxFront> {
    None
}

/// No bundle on this OS.
#[cfg(not(target_os = "macos"))]
#[must_use]
pub fn own_bundle_id() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_cuts_on_char_boundaries() {
        assert_eq!(truncate("héllo wörld", 5), "héllo…");
        assert_eq!(truncate("short", 80), "short");
        assert_eq!(truncate("exactly", 7), "exactly");
        assert_eq!(truncate("trailing space ", 9), "trailing…");
    }

    #[test]
    fn suffix_and_roles() {
        assert_eq!(suffix(true), "accessibility: on");
        assert_eq!(suffix(false), "accessibility: off");
        assert_eq!(role_word("AXLink"), "link");
        assert_eq!(role_word("AXStaticText"), "text");
        assert_eq!(role_word("AXTextField"), "field");
        assert_eq!(role_word("weird"), "weird");
    }

    #[test]
    fn self_is_this_process_or_the_desktop_app() {
        let f = |pid: i32, bundle: Option<&str>| AxFront {
            pid,
            app: String::new(),
            bundle_id: bundle.map(str::to_owned),
            window_title: None,
            document_url: None,
        };
        assert!(is_self(&f(std::process::id() as i32, None)));
        assert!(is_self(&f(1, Some("io.theuth.mac"))));
        assert!(!is_self(&f(1, Some("com.google.Chrome"))));
        assert!(!is_self(&f(1, None)));
    }
}
