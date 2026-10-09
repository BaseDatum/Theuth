//! Accessibility reader (design doc 96 §0.10, §6.2; 149 §7.4 #5). macOS:
//! hit-test the system-wide `AXUIElement` under a point and read the element,
//! the app that owns it and that element's window (title, document URL).
//! Windows: the same through UI Automation (`IUIAutomation::ElementFromPoint`,
//! [`windows_uia`]). Every call is checked, nothing panics on a missing
//! attribute. Linux: `granted()` is `false` and the readers answer `None`
//! until an AT-SPI reader exists.
//!
//! Points are logical global with the origin at the top-left of the primary
//! display — the space `AXUIElementCopyElementAtPosition` hit-tests in. On
//! Windows the displays xcap reports (and so every point the engine receives)
//! are already physical pixels, see [`physical_point`].
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
    /// Windows: the MTA thread holding the `IUIAutomation` instance.
    #[cfg(target_os = "windows")]
    uia: windows_uia::Worker,
}

/// Whether `f` is theuth itself: this process, this process's bundle, or the
/// desktop app (`io.theuth.*`, a separate process from the daemon).
#[must_use]
pub fn is_self(f: &AxFront) -> bool {
    f.pid == std::process::id() as i32
        || f.bundle_id
            .as_deref()
            .is_some_and(|b| b.starts_with("io.theuth.") || Some(b) == own_bundle_id().as_deref())
        || (cfg!(target_os = "windows") && is_theuth_exe(&f.app))
}

/// Whether an executable stem (Windows `app`) is one of theuth's own:
/// `t` (the engine), `theuth-desktop` (the Tauri shell), `Theuth.Companion`.
#[must_use]
pub fn is_theuth_exe(stem: &str) -> bool {
    let s = stem.to_ascii_lowercase();
    s == "t" || s.starts_with("theuth")
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

/// Whether this process may read the tree: `AXIsProcessTrusted` on macOS;
/// always on Windows (UI Automation has no permission — an elevated window
/// refusing a read answers `None` per hit instead, 149 §9).
#[must_use]
pub fn granted() -> bool {
    cfg!(target_os = "windows")
        || (cfg!(target_os = "macos") && crate::input::accessibility_granted())
}

/// Physical/logical ratio of the pointer space on Windows. xcap reports every
/// display from `DEVMODEW` (`dmPosition`, `dmPelsWidth/Height`): physical
/// pixels, so the capture's `Scale::of` is 1 and the samples the companion
/// sends (`GetCursorPos` under PerMonitorV2, 149 §7.4 #4) are physical too.
pub const WINDOWS_CAPTURE_RATIO: f32 = 1.0;

/// Logical global point → physical screen pixel of the display whose logical
/// origin is `origin`, scaled by `ratio` (physical pixels per logical point —
/// the ratio the capture uses for that display), rounded to the nearest pixel.
#[must_use]
pub fn physical_point(p: crate::pointer::Pt, origin: (i32, i32), ratio: f32) -> (i32, i32) {
    let map = |v: f32, o: i32| o.saturating_add(((v - o as f32) * ratio).round() as i32);
    (map(p.x, origin.0), map(p.y, origin.1))
}

/// UI Automation control type id → the role word the `[screen …]` line uses,
/// in the macOS vocabulary where one exists (`link`, `text`, `field`, `button`);
/// else the localized control type lowercased without spaces; else `element`.
/// The ids are UIA's public constants (`UIA_*ControlTypeId`, 50000–50040).
#[must_use]
pub fn uia_role(control_type: i32, localized: Option<&str>) -> String {
    let word = match control_type {
        50000 | 50031 => "button",
        50002 => "checkbox",
        50003 => "combobox",
        50004 => "field",
        50005 => "link",
        50006 => "image",
        50007 => "listitem",
        50008 => "list",
        50009 => "menu",
        50010 => "menubar",
        50011 => "menuitem",
        50012 => "progress",
        50013 => "radio",
        50014 => "scrollbar",
        50015 => "slider",
        50016 => "spinner",
        50017 => "statusbar",
        50018 => "tabgroup",
        50019 => "tab",
        50020 => "text",
        50021 | 50040 => "toolbar",
        50022 => "tooltip",
        50023 => "tree",
        50024 => "treeitem",
        50026 => "group",
        50028 | 50036 => "table",
        50029 => "cell",
        50030 => "document",
        50032 => "window",
        50033 => "pane",
        50034 => "header",
        50035 => "headeritem",
        50037 => "titlebar",
        50038 => "separator",
        _ => "",
    };
    if !word.is_empty() {
        return word.to_owned();
    }
    let fallback: String = localized
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if fallback.is_empty() {
        "element".to_owned()
    } else {
        fallback
    }
}

/// Whether a Windows executable stem is a browser whose window exposes the
/// page URL through UI Automation (the Document's `Value`, else the address bar).
#[must_use]
pub fn is_browser(stem: &str) -> bool {
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "chrome" | "msedge" | "firefox" | "brave" | "opera" | "vivaldi" | "arc" | "chromium"
    )
}

/// Whether an address-bar or document value reads as a URL: no whitespace and
/// a scheme, an `about:` page, or at least one dot.
#[must_use]
pub fn looks_like_url(s: &str) -> bool {
    !s.is_empty()
        && !s.contains(char::is_whitespace)
        && (s.contains("://") || s.starts_with("about:") || s.contains('.'))
}

#[cfg(target_os = "macos")]
pub use macos::{frontmost, own_bundle_id};

#[cfg(target_os = "windows")]
pub use windows_uia::{frontmost, own_bundle_id};

/// UI Automation reader (149 §7.4 #5, §9 Windows). COM lives on one dedicated
/// MTA thread per [`Reader`] ([`Worker`]); the caller's thread (a
/// `spawn_blocking` worker) hands it points over a channel and waits at most
/// [`HIT_BUDGET`] per lookup, so a provider that never answers costs one
/// budget and the rest of `screen_context` goes on without the tree. Inside,
/// `IUIAutomation2` connection/transaction timeouts bound each provider call.
/// Points arrive physical ([`super::physical_point`]); every call is checked,
/// a refused read (UIPI, a dead provider) is `None`, nothing panics.
#[cfg(target_os = "windows")]
mod windows_uia {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
    use std::thread;
    use std::time::Duration;

    use windows::Win32::Foundation::{CloseHandle, POINT};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::System::Variant::{VARIANT, VT_I4};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCondition,
        IUIAutomationElement, IUIAutomationTreeWalker, IUIAutomationValuePattern,
        TreeScope_Descendants, UIA_CONTROLTYPE_ID, UIA_ControlTypePropertyId,
        UIA_DocumentControlTypeId, UIA_EditControlTypeId, UIA_HyperlinkControlTypeId,
        UIA_PaneControlTypeId, UIA_ValuePatternId, UIA_WindowControlTypeId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };
    use windows::core::{Interface, PWSTR};

    use super::{
        AxElement, AxFront, AxHit, Reader, WINDOWS_CAPTURE_RATIO, is_browser, looks_like_url,
        uia_role,
    };
    use crate::pointer::Pt;

    /// Wall-clock bound on one lookup, whatever UIA does inside.
    pub(super) const HIT_BUDGET: Duration = Duration::from_millis(2000);
    /// `IUIAutomation2::ConnectionTimeout`: connecting to a provider, ms.
    pub(super) const CONNECTION_TIMEOUT_MS: u32 = 1000;
    /// `IUIAutomation2::TransactionTimeout`: one provider call, ms.
    pub(super) const TRANSACTION_TIMEOUT_MS: u32 = 1000;
    /// How far the control view is walked up looking for the top-level window.
    const PARENT_DEPTH: usize = 32;
    /// Retries of a hit on a bare Chromium pane, and the pause between them
    /// (the macOS reader's `MANUAL_RETRIES` / `MANUAL_PAUSE`).
    const CHROMIUM_RETRIES: usize = 3;
    const CHROMIUM_PAUSE: Duration = Duration::from_millis(120);
    /// The window class of Chromium's content area.
    const CHROMIUM_CONTENT_CLASS: &str = "Chrome_RenderWidgetHostHWND";
    /// Longest window title read back through `GetWindowTextW`.
    const TITLE_CHARS: usize = 512;

    enum Job {
        Hit((i32, i32), SyncSender<Option<AxHit>>),
        Front(SyncSender<Option<AxFront>>),
    }

    /// The MTA thread holding the `IUIAutomation` instance: spawned on the first
    /// lookup, abandoned (never joined, its sender dropped so it exits when the
    /// stuck call returns) after a lookup overruns [`HIT_BUDGET`]; every later
    /// lookup of this reader answers `None` at once.
    #[derive(Debug, Default)]
    pub(super) struct Worker {
        tx: Option<Sender<Job>>,
        dead: bool,
    }

    impl Worker {
        fn sender(&mut self) -> Option<&Sender<Job>> {
            if self.dead {
                return None;
            }
            if self.tx.is_none() {
                let (tx, rx) = mpsc::channel();
                let spawned = thread::Builder::new()
                    .name("uia-reader".to_owned())
                    .spawn(move || serve(rx));
                if let Err(e) = spawned {
                    tracing::warn!(%e, "uia reader thread did not start");
                    self.dead = true;
                    return None;
                }
                self.tx = Some(tx);
            }
            self.tx.as_ref()
        }

        fn call<T>(&mut self, job: impl FnOnce(SyncSender<Option<T>>) -> Job) -> Option<T> {
            let (reply, rx) = mpsc::sync_channel(1);
            if self.sender()?.send(job(reply)).is_err() {
                self.dead = true;
                self.tx = None;
                return None;
            }
            match rx.recv_timeout(HIT_BUDGET) {
                Ok(v) => v,
                Err(RecvTimeoutError::Timeout) => {
                    tracing::warn!(
                        budget_ms = HIT_BUDGET.as_millis() as u64,
                        "uia lookup overran its budget; reading no more this call"
                    );
                    self.dead = true;
                    self.tx = None;
                    None
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.dead = true;
                    self.tx = None;
                    None
                }
            }
        }

        /// The hit under a physical screen pixel.
        pub(super) fn hit(&mut self, p: (i32, i32)) -> Option<AxHit> {
            self.call(|reply| Job::Hit(p, reply))
        }

        /// The foreground window's app and title.
        pub(super) fn front(&mut self) -> Option<AxFront> {
            self.call(Job::Front)
        }
    }

    /// The worker thread: COM MTA, one `Uia`, jobs until every sender is gone.
    fn serve(rx: Receiver<Job>) {
        // SAFETY: a fresh thread with no COM state; balanced by `CoUninitialize` below.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            tracing::debug!(?hr, "CoInitializeEx failed; accessibility off");
            return;
        }
        let uia = Uia::new();
        for job in rx {
            match job {
                Job::Hit(p, reply) => {
                    let _ = reply.send(uia.as_ref().and_then(|u| u.hit(p)));
                }
                Job::Front(reply) => {
                    let _ = reply.send(uia.as_ref().and_then(Uia::front));
                }
            }
        }
        drop(uia);
        // SAFETY: balances the successful `CoInitializeEx` above; every interface was dropped.
        unsafe { CoUninitialize() };
    }

    struct Uia {
        auto: IUIAutomation,
        walker: IUIAutomationTreeWalker,
        root: IUIAutomationElement,
        /// Processes whose bare Chromium pane was already retried.
        retried: RefCell<HashSet<i32>>,
    }

    impl Uia {
        fn new() -> Option<Self> {
            // SAFETY: COM is initialised on this thread; plain class instantiation.
            let created: windows::core::Result<IUIAutomation> =
                unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
                    // SAFETY: as above; the Windows 7 class when the Windows 8 one is missing.
                    .or_else(|_| unsafe {
                        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                    });
            let auto = match created {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!(%e, "CUIAutomation unavailable; accessibility off");
                    return None;
                }
            };
            if let Ok(a2) = auto.cast::<IUIAutomation2>() {
                // SAFETY: live interface, plain setters.
                unsafe {
                    let _ = a2.SetConnectionTimeout(CONNECTION_TIMEOUT_MS);
                    let _ = a2.SetTransactionTimeout(TRANSACTION_TIMEOUT_MS);
                }
            }
            // SAFETY: live interface.
            let walker = unsafe { auto.ControlViewWalker() }.ok()?;
            // SAFETY: live interface.
            let root = unsafe { auto.GetRootElement() }.ok()?;
            Some(Self {
                auto,
                walker,
                root,
                retried: RefCell::new(HashSet::new()),
            })
        }

        /// The element under a physical screen pixel and the app/window owning it.
        /// Chromium content (browsers, Electron, WebView2) answers a bare
        /// `Chrome_RenderWidgetHostHWND` pane until its accessibility tree is
        /// built, which the first UIA query itself switches on: such a hit is
        /// retried up to [`CHROMIUM_RETRIES`] times, once per process per reader.
        fn hit(&self, (x, y): (i32, i32)) -> Option<AxHit> {
            // SAFETY: live interface; `POINT` by value.
            let mut el = unsafe { self.auto.ElementFromPoint(POINT { x, y }) }.ok()?;
            // SAFETY: live element.
            let pid = unsafe { el.CurrentProcessId() }.ok().filter(|p| *p > 0);
            let mut ct = control_type(&el);
            if let Some(pid) = pid
                && bare_chromium(&el, ct)
                && self.retried.borrow_mut().insert(pid)
            {
                tracing::debug!(
                    pid,
                    "bare Chromium pane under the pointer; retrying the hit"
                );
                for _ in 0..CHROMIUM_RETRIES {
                    thread::sleep(CHROMIUM_PAUSE);
                    // SAFETY: as above.
                    let Ok(again) = (unsafe { self.auto.ElementFromPoint(POINT { x, y }) }) else {
                        break;
                    };
                    let again_ct = control_type(&again);
                    let done = !bare_chromium(&again, again_ct);
                    (el, ct) = (again, again_ct);
                    if done {
                        break;
                    }
                }
            }
            let top = self.top_window(&el);
            let window_level =
                ct == UIA_WindowControlTypeId.0 || top.as_ref().is_some_and(|w| self.same(w, &el));
            // The app is the top-level window's process: web content belongs to a
            // WebView2/Chromium child process (`msedgewebview2`), the window to the app.
            let owner = top
                .as_ref()
                // SAFETY: live element.
                .and_then(|w| unsafe { w.CurrentProcessId() }.ok().filter(|p| *p > 0))
                .or(pid);
            let front = owner.map(|pid| self.front_of(pid, top.as_ref()));
            let element = (!window_level).then(|| element_of(&el, ct));
            Some(AxHit { element, front })
        }

        /// The foreground window as `AxFront` — the last resort when no hit resolved an app.
        fn front(&self) -> Option<AxFront> {
            // SAFETY: plain query.
            let hwnd = unsafe { GetForegroundWindow() };
            if hwnd.is_invalid() {
                return None;
            }
            // SAFETY: a window handle the OS just reported; a stale one is an error, not UB.
            let el = unsafe { self.auto.ElementFromHandle(hwnd) }.ok()?;
            let mut owner = 0u32;
            // SAFETY: `owner` is a valid slot for the process id.
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
            // SAFETY: live element.
            let pid = unsafe { el.CurrentProcessId() }
                .ok()
                .filter(|p| *p > 0)
                .or_else(|| i32::try_from(owner).ok().filter(|p| *p > 0))?;
            Some(self.front_of(pid, Some(&el)))
        }

        /// Whether `a` and `b` are the same element (`CompareElements`).
        fn same(&self, a: &IUIAutomationElement, b: &IUIAutomationElement) -> bool {
            // SAFETY: two live elements.
            unsafe { self.auto.CompareElements(a, b) }.is_ok_and(|same| same.as_bool())
        }

        /// The top-level window holding `el`: the control-view ancestor whose
        /// parent is the desktop root (`el` itself when it is one). `None` for
        /// the root and when the walk fails or runs past [`PARENT_DEPTH`].
        fn top_window(&self, el: &IUIAutomationElement) -> Option<IUIAutomationElement> {
            let mut cur = el.clone();
            for _ in 0..PARENT_DEPTH {
                // SAFETY: live walker and element.
                let parent = unsafe { self.walker.GetParentElement(&cur) }.ok()?;
                if self.same(&parent, &self.root) {
                    return Some(cur);
                }
                cur = parent;
            }
            None
        }

        fn front_of(&self, pid: i32, window: Option<&IUIAutomationElement>) -> AxFront {
            let app = u32::try_from(pid)
                .ok()
                .and_then(exe_stem)
                .unwrap_or_default();
            let window_title = window.and_then(window_title);
            let document_url = window
                .filter(|_| is_browser(&app))
                .and_then(|w| self.document_url(w));
            AxFront {
                pid,
                app,
                bundle_id: None,
                window_title,
                document_url,
            }
        }

        /// A browser window's page URL: the first Document's `Value` (Chromium
        /// and Firefox put the URL there), else the first Edit's (the address
        /// bar). Each search is one `FindFirst` bounded by the transaction timeout.
        fn document_url(&self, window: &IUIAutomationElement) -> Option<String> {
            [UIA_DocumentControlTypeId, UIA_EditControlTypeId]
                .into_iter()
                .find_map(|ct| {
                    let cond = self.control_type_is(ct)?;
                    // SAFETY: live element and condition; bounded by the transaction timeout.
                    let found = unsafe { window.FindFirst(TreeScope_Descendants, &cond) }.ok()?;
                    value(&found).filter(|v| looks_like_url(v))
                })
        }

        /// `ControlType == ct` as a property condition.
        fn control_type_is(&self, ct: UIA_CONTROLTYPE_ID) -> Option<IUIAutomationCondition> {
            let mut v = VARIANT::default();
            // SAFETY: a zeroed VARIANT; the tag and the matching union arm are set together.
            unsafe {
                let inner = &mut *v.Anonymous.Anonymous;
                inner.vt = VT_I4;
                inner.Anonymous.lVal = ct.0;
            }
            // SAFETY: live interface; `v` is a complete VT_I4 variant it copies.
            unsafe {
                self.auto
                    .CreatePropertyCondition(UIA_ControlTypePropertyId, &v)
            }
            .ok()
        }
    }

    fn clean(s: String) -> Option<String> {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_owned())
    }

    /// A nameless pane whose class is Chromium's content host: the tree is not
    /// built yet (see [`Uia::hit`]).
    fn bare_chromium(el: &IUIAutomationElement, ct: i32) -> bool {
        (ct == UIA_PaneControlTypeId.0 || ct == 0)
            && name(el).is_none()
            // SAFETY: live element.
            && unsafe { el.CurrentClassName() }
                .is_ok_and(|c| c.to_string() == CHROMIUM_CONTENT_CLASS)
    }

    fn control_type(el: &IUIAutomationElement) -> i32 {
        // SAFETY: live element.
        unsafe { el.CurrentControlType() }.map_or(0, |c| c.0)
    }

    fn name(el: &IUIAutomationElement) -> Option<String> {
        // SAFETY: live element.
        unsafe { el.CurrentName() }
            .ok()
            .and_then(|b| clean(b.to_string()))
    }

    fn localized_type(el: &IUIAutomationElement) -> Option<String> {
        // SAFETY: live element.
        unsafe { el.CurrentLocalizedControlType() }
            .ok()
            .and_then(|b| clean(b.to_string()))
    }

    /// `ValuePattern.Value` when the element supports it.
    fn value(el: &IUIAutomationElement) -> Option<String> {
        // SAFETY: live element; the pattern interface is released with the binding.
        let pattern: IUIAutomationValuePattern =
            unsafe { el.GetCurrentPatternAs(UIA_ValuePatternId) }.ok()?;
        // SAFETY: live pattern.
        unsafe { pattern.CurrentValue() }
            .ok()
            .and_then(|b| clean(b.to_string()))
    }

    /// The window element's `Name`, else `GetWindowTextW` on its HWND.
    fn window_title(w: &IUIAutomationElement) -> Option<String> {
        name(w).or_else(|| {
            // SAFETY: live element.
            let hwnd = unsafe { w.CurrentNativeWindowHandle() }
                .ok()
                .filter(|h| !h.is_invalid())?;
            let mut buf = [0u16; TITLE_CHARS];
            // SAFETY: `buf` is the slice the call writes into, length-checked by the binding.
            let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
            let n = usize::try_from(n).ok()?.min(buf.len());
            clean(String::from_utf16_lossy(&buf[..n]))
        })
    }

    fn element_of(el: &IUIAutomationElement, ct: i32) -> AxElement {
        let label = name(el);
        let value = value(el);
        let (value, url) =
            if ct == UIA_HyperlinkControlTypeId.0 && value.as_deref().is_some_and(looks_like_url) {
                (None, value)
            } else {
                (value, None)
            };
        AxElement {
            role: uia_role(ct, localized_type(el).as_deref()),
            label,
            value,
            url,
        }
    }

    /// The process's executable stem (`msedge`, `Code`, `explorer`) via
    /// `QueryFullProcessImageNameW`; `None` when the process cannot be opened.
    fn exe_stem(pid: u32) -> Option<String> {
        // SAFETY: plain handle open; closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut buf = [0u16; 1024];
        let mut len = u32::try_from(buf.len()).ok()?;
        // SAFETY: `buf` holds `len` UTF-16 units; the call writes at most that many and
        // updates `len` to the count written.
        let queried = unsafe {
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
        };
        // SAFETY: `h` came from `OpenProcess` and is not used after this.
        let _ = unsafe { CloseHandle(h) };
        queried.ok()?;
        let n = usize::try_from(len).ok()?.min(buf.len());
        let path = String::from_utf16_lossy(&buf[..n]);
        Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
    }

    impl Reader {
        /// The element under `global` and the app/window owning it.
        #[must_use]
        pub fn hit_at(&mut self, global: Pt) -> Option<AxHit> {
            let p = super::physical_point(global, (0, 0), WINDOWS_CAPTURE_RATIO);
            self.uia.hit(p)
        }
    }

    /// Windows has no bundle ids; `is_self` matches theuth's executable stems instead.
    #[must_use]
    pub fn own_bundle_id() -> Option<String> {
        None
    }

    /// The foreground window's app and title, on a one-shot UIA thread.
    #[must_use]
    pub fn frontmost() -> Option<AxFront> {
        Worker::default().front()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A real desktop session: the centre of the primary display has something
        /// under it (the wallpaper's pane at the least). Needs an interactive
        /// window station — run by hand: `cargo test -p computer --lib -- --ignored windows_hit`.
        #[test]
        #[ignore = "needs an interactive desktop session"]
        fn windows_hit_tests_the_primary_display_centre() {
            let displays = crate::screen::displays().expect("displays");
            let d = displays
                .iter()
                .find(|d| d.main)
                .or_else(|| displays.first())
                .expect("a display");
            let centre = Pt {
                x: d.x as f32 + d.width as f32 / 2.0,
                y: d.y as f32 + d.height as f32 / 2.0,
            };
            let mut reader = Reader::default();
            let started = std::time::Instant::now();
            let hit = reader
                .hit_at(centre)
                .expect("a hit under the display centre");
            let took = started.elapsed();
            eprintln!("hit at {centre:?} in {took:?}: {hit:?}");
            assert!(took <= HIT_BUDGET, "hit took {took:?}");
            let front = hit.front.expect("an owning process");
            assert!(front.pid > 0);
            assert!(
                !front.app.is_empty() || front.window_title.is_some(),
                "neither app nor title resolved: {front:?}"
            );
            let front = frontmost();
            eprintln!("frontmost: {front:?}");
            // Diagnostics: the control-view chain from the hit up to the root, on this thread.
            // SAFETY: the test thread has no COM state yet; balanced below.
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            assert!(hr.is_ok(), "CoInitializeEx: {hr:?}");
            {
                let uia = Uia::new().expect("IUIAutomation");
                let (x, y) = super::super::physical_point(centre, (0, 0), WINDOWS_CAPTURE_RATIO);
                // SAFETY: live interface.
                let mut cur = unsafe { uia.auto.ElementFromPoint(POINT { x, y }) }.ok();
                let mut depth = 0;
                while let Some(el) = cur {
                    // SAFETY: live element.
                    let (pid, hwnd, class) = unsafe {
                        (
                            el.CurrentProcessId().ok(),
                            el.CurrentNativeWindowHandle().ok(),
                            el.CurrentClassName().ok().map(|b| b.to_string()),
                        )
                    };
                    eprintln!(
                        "  [{depth}] type={} name={:?} class={class:?} pid={pid:?} hwnd={hwnd:?} text={:?} root={}",
                        control_type(&el),
                        name(&el),
                        hwnd.filter(|h| !h.is_invalid()).and_then(|h| {
                            let mut buf = [0u16; TITLE_CHARS];
                            // SAFETY: `buf` is the slice written into.
                            let n = unsafe { GetWindowTextW(h, &mut buf) };
                            clean(String::from_utf16_lossy(
                                &buf[..usize::try_from(n).unwrap_or(0)],
                            ))
                        }),
                        uia.same(&el, &uia.root),
                    );
                    depth += 1;
                    if depth > PARENT_DEPTH {
                        break;
                    }
                    // SAFETY: live walker and element.
                    cur = unsafe { uia.walker.GetParentElement(&el) }.ok();
                }
            }
            // SAFETY: balances the CoInitializeEx above; every interface was dropped.
            unsafe { CoUninitialize() };
        }

        /// No desktop needed: COM, the automation object and the root element exist,
        /// and a reader never panics or overruns its budget on a point off every display.
        #[test]
        fn windows_reader_is_bounded_and_quiet_off_screen() {
            assert!(super::super::granted());
            let mut reader = Reader::default();
            let started = std::time::Instant::now();
            let hit = reader.hit_at(Pt {
                x: -100_000.0,
                y: -100_000.0,
            });
            assert!(started.elapsed() <= HIT_BUDGET + Duration::from_millis(500));
            // Off-screen points answer the root (no window → no element) or nothing.
            if let Some(h) = hit {
                assert!(h.element.is_none() || h.front.is_some(), "{h:?}");
            }
        }
    }
}

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

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl Reader {
    /// No reader on this OS.
    #[must_use]
    pub fn hit_at(&mut self, _global: crate::pointer::Pt) -> Option<AxHit> {
        None
    }
}

/// No reader on this OS.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[must_use]
pub fn frontmost() -> Option<AxFront> {
    None
}

/// No bundle on this OS.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
        assert_eq!(
            is_self(&AxFront {
                app: "Theuth.Companion".into(),
                ..f(1, None)
            }),
            cfg!(target_os = "windows")
        );
    }

    #[test]
    fn theuth_exe_stems() {
        assert!(is_theuth_exe("t"));
        assert!(is_theuth_exe("theuth-desktop"));
        assert!(is_theuth_exe("Theuth.Companion"));
        assert!(!is_theuth_exe("msedge"));
        assert!(!is_theuth_exe("tt"));
        assert!(!is_theuth_exe(""));
    }

    #[test]
    fn physical_point_scales_from_the_display_origin() {
        use crate::pointer::Pt;
        // Windows: xcap's DEVMODE geometry is physical, ratio 1 → identity (rounded).
        assert_eq!(WINDOWS_CAPTURE_RATIO, 1.0);
        assert_eq!(
            physical_point(Pt { x: 612.4, y: 418.6 }, (0, 0), WINDOWS_CAPTURE_RATIO),
            (612, 419)
        );
        assert_eq!(
            physical_point(
                Pt {
                    x: 1612.0,
                    y: -100.0
                },
                (1512, -200),
                WINDOWS_CAPTURE_RATIO
            ),
            (1612, -100)
        );
        // A hypothetical logical display at 2× maps offsets from its origin.
        assert_eq!(
            physical_point(
                Pt {
                    x: 1612.0,
                    y: -100.0
                },
                (1512, -200),
                2.0
            ),
            (1712, 0)
        );
        assert_eq!(
            physical_point(Pt { x: 10.25, y: 10.75 }, (0, 0), 2.0),
            (21, 22)
        );
        // 150 %: a point one logical unit in lands 1.5 px in, rounded half away from zero.
        assert_eq!(physical_point(Pt { x: 1.0, y: 3.0 }, (0, 0), 1.5), (2, 5));
    }

    #[test]
    fn uia_roles_use_the_line_vocabulary() {
        assert_eq!(uia_role(50005, Some("hyperlink")), "link");
        assert_eq!(uia_role(50020, Some("text")), "text");
        assert_eq!(uia_role(50004, Some("edit")), "field");
        assert_eq!(uia_role(50000, Some("button")), "button");
        assert_eq!(uia_role(50032, None), "window");
        assert_eq!(uia_role(50030, None), "document");
        // Custom and unknown ids fall back to the localized type, squashed and lowercased.
        assert_eq!(uia_role(50025, Some("Tab Item")), "tabitem");
        assert_eq!(uia_role(50025, Some("Schaltfläche")), "schaltfläche");
        assert_eq!(uia_role(0, None), "element");
        assert_eq!(uia_role(0, Some("   ")), "element");
    }

    #[test]
    fn browsers_and_urls() {
        assert!(is_browser("msedge"));
        assert!(is_browser("Chrome"));
        assert!(is_browser("firefox"));
        assert!(!is_browser("Code"));
        assert!(!is_browser("explorer"));
        assert!(looks_like_url("https://github.com/basedatum/theuth"));
        assert!(looks_like_url("github.com/basedatum"));
        assert!(looks_like_url("about:blank"));
        assert!(!looks_like_url("Search or enter web address"));
        assert!(!looks_like_url("hello"));
        assert!(!looks_like_url(""));
    }
}
