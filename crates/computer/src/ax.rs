//! Accessibility reader (design doc 96 §0.10, §6.2). macOS: hit-test the
//! system-wide `AXUIElement` under a point and read the frontmost app, its
//! focused window title and document URL. Every call is checked, nothing
//! panics on a missing attribute. Other OSes: `granted()` is `false` and the
//! readers answer `None` until UIAutomation/AT-SPI readers exist.
//!
//! Points are logical global with the origin at the top-left of the primary
//! display — the space `AXUIElementCopyElementAtPosition` hit-tests in.

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

/// The frontmost application and its focused window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxFront {
    /// `NSRunningApplication.localizedName`.
    pub app: String,
    /// `NSRunningApplication.bundleIdentifier`.
    pub bundle_id: Option<String>,
    /// `AXFocusedWindow` → `AXTitle`.
    pub window_title: Option<String>,
    /// `AXFocusedWindow` → `AXDocument`, else `AXURL`.
    pub document_url: Option<String>,
}

/// Cut `s` to at most `chars` characters on a char boundary, marking the cut with `…`.
#[must_use]
pub fn truncate(s: &str, chars: usize) -> String {
    match s.char_indices().nth(chars) {
        Some((i, _)) => format!("{}…", &s[..i].trim_end()),
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
pub use macos::{element_at, frontmost};

#[cfg(target_os = "macos")]
mod macos {
    use std::ptr::{NonNull, null};

    use objc2_app_kit::NSWorkspace;
    use objc2_application_services::{AXError, AXUIElement};
    use objc2_core_foundation::{CFBoolean, CFNumber, CFRetained, CFString, CFType, CFURL};

    use super::{AxElement, AxFront, role_word};
    use crate::pointer::Pt;

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

    /// The element under `global`, read from the system-wide element.
    #[must_use]
    pub fn element_at(global: Pt) -> Option<AxElement> {
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
        let el = NonNull::new(out.cast_mut()).map(|p| unsafe { CFRetained::from_raw(p) })?;
        Some(AxElement {
            role: role_word(&string_attr(&el, "AXRole").unwrap_or_else(|| "element".to_owned())),
            label: string_attr(&el, "AXTitle").or_else(|| string_attr(&el, "AXDescription")),
            value: string_attr(&el, "AXValue"),
            url: string_attr(&el, "AXURL"),
        })
    }

    /// The frontmost app via `NSWorkspace`, then its focused window via AX.
    #[must_use]
    pub fn frontmost() -> Option<AxFront> {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let name = app
            .localizedName()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let bundle_id = app.bundleIdentifier().map(|s| s.to_string());
        // SAFETY: plain constructor over a pid the OS just reported.
        let ax_app = unsafe { AXUIElement::new_application(app.processIdentifier()) };
        let window = element_attr(&ax_app, "AXFocusedWindow");
        let (window_title, document_url) = match window {
            Some(w) => (
                string_attr(&w, "AXTitle"),
                string_attr(&w, "AXDocument").or_else(|| string_attr(&w, "AXURL")),
            ),
            None => (None, None),
        };
        Some(AxFront {
            app: name,
            bundle_id,
            window_title,
            document_url,
        })
    }
}

/// No reader on this OS.
#[cfg(not(target_os = "macos"))]
#[must_use]
pub fn element_at(_global: crate::pointer::Pt) -> Option<AxElement> {
    None
}

/// No reader on this OS.
#[cfg(not(target_os = "macos"))]
#[must_use]
pub fn frontmost() -> Option<AxFront> {
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
}
