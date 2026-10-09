import Foundation
import TheuthKit

/// What the companion read from the focused element at the press (153 §13.1). D2 fills it from AX
/// (`kAXFocusedUIElement`: role, subrole, `AXUIElementIsAttributeSettable(kAXValue)`, value / selected-range presence, the
/// owning app's bundle id and window title); Windows fills it from UIA with the `uia_role` names. Pure value, no AX refs.
public struct AXSnapshot: Equatable, Sendable {
    /// `kAXRole` (`"AXTextField"`, …); `nil` = no focused element readable (GPU terminals, VMs, games, no grant).
    public var role: String?
    /// `kAXSubrole` (`"AXSearchField"`, `"AXSecureTextField"`, …).
    public var subrole: String?
    /// `kAXValue` settable; `nil` = could not ask.
    public var isEditable: Bool?
    public var hasValue: Bool
    public var hasSelectedTextRange: Bool
    /// `AXSecureTextField` / UIA `IsPassword`.
    public var isSecure: Bool
    public var appBundleId: String?
    public var windowTitle: String?

    public init(role: String? = nil, subrole: String? = nil, isEditable: Bool? = nil, hasValue: Bool = false,
                hasSelectedTextRange: Bool = false, isSecure: Bool = false, appBundleId: String? = nil, windowTitle: String? = nil) {
        self.role = role; self.subrole = subrole; self.isEditable = isEditable; self.hasValue = hasValue
        self.hasSelectedTextRange = hasSelectedTextRange; self.isSecure = isSecure
        self.appBundleId = appBundleId; self.windowTitle = windowTitle
    }

    /// Secure by flag or by role/subrole — the one question D2 asks before "send it" presses Return (§7.4, §13.8).
    public var secure: Bool { isSecure || role == "AXSecureTextField" || subrole == "AXSecureTextField" }
}

/// §13.1's three-way verdict: refuse only when certain; everything else gets the lease and the verification.
public enum DeliveryAssessment {
    public enum Verdict: Equatable, Sendable {
        /// Direct insertion (W-m) first, the lease when the read-back fails.
        case editable
        /// Refuse before the mic opens: hint "Nothing to type into — ‹role›" (`Refusal.label`).
        case certainlyNot(Refusal)
        /// Clipboard lease + verification.
        case unknown
    }

    public enum Refusal: Equatable, Sendable {
        /// A role from the "certainly not" row; the payload is the AX role name (`"AXButton"`).
        case role(String)
        /// Return into a secure field (§7.4): the text is delivered, the Return is not.
        case secure
    }

    /// What is about to be delivered: words, or the Return "send it" presses (§7.4).
    public enum Payload: Equatable, Sendable { case text, returnKey }

    /// Roles that are text fields by nature (first row of the table).
    public static let editableRoles: Set<String> = ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField", "AXSecureTextField"]
    /// Roles that are certainly not text targets (third row); `AXMenu*` is matched by prefix, a table row is `AXRow`.
    public static let refusedRoles: Set<String> = ["AXButton", "AXStaticText", "AXSlider", "AXCheckBox", "AXRadioButton", "AXImage",
                                                   "AXToolbar", "AXRow", "AXPopUpButton", "AXLink", "AXDisclosureTriangle"]

    /// The verdict for `snapshot`. `terminal` = the resolved `AppProfile.terminal` (Terminal.app, GPU terminals, the editors'
    /// integrated terminals): an editable role there is downgraded to `unknown` so the lease is used (§13.2 — no direct
    /// insertion into terminals / web views).
    public static func assess(_ snapshot: AXSnapshot, payload: Payload = .text, terminal: Bool = false) -> Verdict {
        if payload == .returnKey, snapshot.secure { return .certainlyNot(.secure) }
        guard let role = snapshot.role else { return .unknown }
        if refusedRoles.contains(role) || role.hasPrefix("AXMenu") { return .certainlyNot(.role(role)) }
        let textRole = editableRoles.contains(role) || snapshot.subrole.map(editableRoles.contains) == true || snapshot.secure
        if textRole {
            // A text role whose value is reported non-settable (a read-only field): not certain enough to refuse — the lease
            // and the verification decide.
            if snapshot.isEditable == false { return .unknown }
            return terminal ? .unknown : .editable
        }
        // Any other role whose `kAXValue` is settable (table: "or kAXValue settable") — still not for terminals.
        if snapshot.isEditable == true, !terminal { return .editable }
        return .unknown
    }

    /// The human word of the hint "Nothing to type into — ‹role›": `"AXStaticText"` → "static text", `"AXMenuItem"` → "menu item".
    public static func label(_ refusal: Refusal) -> String {
        switch refusal {
        case .secure: return "secure field"
        case .role(let role):
            let bare = role.hasPrefix("AX") ? String(role.dropFirst(2)) : role
            var out = ""
            for ch in bare {
                if ch.isUppercase, !out.isEmpty { out.append(" ") }
                out.append(contentsOf: ch.lowercased())
            }
            return out
        }
    }
}
