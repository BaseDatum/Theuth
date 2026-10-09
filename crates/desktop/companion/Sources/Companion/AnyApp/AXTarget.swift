import AppKit
import ApplicationServices
import CompanionCore

/// 153 §13.1: the focused element captured once at the press (pid, element, app), read into D1's `AXSnapshot`, then used
/// for direct insertion (§13.2), the verification reads (§13.4), focus restore (§13.6) and "scratch that". Every AX call is
/// synchronous and local; nothing here is called from a probe.
struct AXTarget {
    let app: NSRunningApplication
    let element: AXUIElement
    var pid: pid_t { app.processIdentifier }

    /// `AXUIElementCreateSystemWide` → focused application → focused UI element. `nil` target when nothing is focused or
    /// the grant is missing; the snapshot is then `AXSnapshot()` (→ `unknown`, lease) with the frontmost app's identity.
    @MainActor
    static func capture() -> (target: AXTarget?, snapshot: AXSnapshot, selection: String?) {
        let frontmost = NSWorkspace.shared.frontmostApplication
        var snapshot = AXSnapshot(appBundleId: frontmost?.bundleIdentifier)
        let sys = AXUIElementCreateSystemWide()
        guard let focused = sys.attribute(kAXFocusedUIElementAttribute), CFGetTypeID(focused) == AXUIElementGetTypeID() else {
            return (frontmost.map { AXTarget(app: $0, element: AXUIElementCreateApplication($0.processIdentifier)) }, snapshot, nil)
        }
        let element = unsafeDowncast(focused, to: AXUIElement.self)
        var pid: pid_t = 0
        AXUIElementGetPid(element, &pid)
        let app = NSRunningApplication(processIdentifier: pid) ?? frontmost
        snapshot.appBundleId = app?.bundleIdentifier ?? snapshot.appBundleId
        snapshot.role = element.string(kAXRoleAttribute)
        snapshot.subrole = element.string(kAXSubroleAttribute)
        var settable = DarwinBoolean(false)
        if AXUIElementIsAttributeSettable(element, kAXValueAttribute as CFString, &settable) == .success { snapshot.isEditable = settable.boolValue }
        snapshot.hasValue = element.attribute(kAXValueAttribute) != nil
        snapshot.hasSelectedTextRange = element.range(kAXSelectedTextRangeAttribute) != nil
        snapshot.isSecure = snapshot.role == "AXSecureTextField" || snapshot.subrole == "AXSecureTextField"
        if let w = element.attribute(kAXWindowAttribute), CFGetTypeID(w) == AXUIElementGetTypeID() {
            snapshot.windowTitle = unsafeDowncast(w, to: AXUIElement.self).string(kAXTitleAttribute)
        }
        // §13.6: a non-empty selection enters rewrite mode; a secure field's selection is never read.
        let selection = snapshot.secure ? nil : element.string(kAXSelectedTextAttribute).flatMap { $0.isEmpty ? nil : $0 }
        return (app.map { AXTarget(app: $0, element: element) }, snapshot, selection)
    }

    /// The system-wide focus is still our element (CFEqual on the AX reference).
    var stillFocused: Bool {
        guard let f = AXUIElementCreateSystemWide().attribute(kAXFocusedUIElementAttribute), CFGetTypeID(f) == AXUIElementGetTypeID() else { return false }
        return CFEqual(f, element)
    }

    /// §13.4: value (≤ 60 000 chars), count, caret; `nil` when the focus moved or the app is gone.
    func read() -> FieldRead? {
        guard !app.isTerminated, stillFocused else { return nil }
        let value = element.string(kAXValueAttribute).map { $0.count > PasteVerifier.maxValueChars ? String($0.prefix(PasteVerifier.maxValueChars)) : $0 }
        let count = element.attribute(kAXNumberOfCharactersAttribute).flatMap { $0 as? Int }
        return FieldRead(value: value, count: count, caret: element.range(kAXSelectedTextRangeAttribute)?.location)
    }

    /// §13.2: `kAXSelectedText` write — replaces the selection or inserts at the caret.
    func insert(_ text: String) -> Bool {
        AXUIElementSetAttributeValue(element, kAXSelectedTextAttribute as CFString, text as CFString) == .success
    }

    /// "scratch that" on a direct insertion: select the `chars` before the caret and replace them with nothing.
    func selectBack(_ chars: Int) -> Bool {
        guard let r = element.range(kAXSelectedTextRangeAttribute), r.location >= chars else { return false }
        var range = CFRange(location: r.location - chars, length: chars)
        guard let v = AXValueCreate(.cfRange, &range), AXUIElementSetAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, v) == .success else { return false }
        return insert("")
    }

    /// §13.6 Replace: the app to the front, then the element focused (hiding our panel alone races the paste).
    func focus() {
        app.activate()
        AXUIElementSetAttributeValue(element, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    }
}

extension AXUIElement {
    func attribute(_ name: String) -> CFTypeRef? {
        var out: CFTypeRef?
        return AXUIElementCopyAttributeValue(self, name as CFString, &out) == .success ? out : nil
    }

    func string(_ name: String) -> String? { attribute(name) as? String }

    func range(_ name: String) -> CFRange? {
        guard let v = attribute(name), CFGetTypeID(v) == AXValueGetTypeID() else { return nil }
        var r = CFRange()
        return AXValueGetValue(unsafeDowncast(v, to: AXValue.self), .cfRange, &r) ? r : nil
    }
}

/// §13.8 / §13.7: the two system facts read at the press.
enum SystemState {
    /// `CGSessionCopyCurrentDictionary` → `CGSSessionScreenIsLocked`.
    static var screenLocked: Bool {
        guard let d = CGSessionCopyCurrentDictionary() as? [String: Any] else { return false }
        return (d["CGSSessionScreenIsLocked"] as? Bool) ?? false
    }

    static var accessibilityTrusted: Bool { AXIsProcessTrusted() }
}
