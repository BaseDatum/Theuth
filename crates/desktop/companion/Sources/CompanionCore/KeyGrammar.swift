import Foundation
import TheuthKit

/// A key event stripped of AppKit: the typed characters, the virtual key code and the ⌘ flag.
public struct KeyPress: Equatable, Sendable {
    public var chars: String
    public var keyCode: UInt16
    public var command: Bool
    public init(chars: String, keyCode: UInt16 = 0, command: Bool = false) {
        self.chars = chars; self.keyCode = keyCode; self.command = command
    }
}

/// What a keystroke means in the panel (130 §4.2 table). No side effects — the app maps intents to actions.
public enum Intent: Equatable, Sendable {
    /// 1 · 2 · 3 (1-based).
    case answer(Int)
    /// ⏎: the recommended option.
    case answerRecommended
    /// ⎋: undo during the hold, otherwise close.
    case escape
    case next, previous
    /// S: snooze 30 m.
    case snooze
    /// E: clear a finished receipt.
    case clear
    /// D · O · ⌘O: deep links into the main window.
    case openDiff, openBoard, openSession
    /// R: reply in words (W3); ⌘L: jump to the Ask bar (W3).
    case reply, askBar
    case none
}

public enum KeyGrammar {
    public static let snoozeInterval: TimeInterval = 30 * 60

    // Virtual key codes (Carbon HIToolbox).
    static let kReturn: UInt16 = 36, kKeypadEnter: UInt16 = 76, kEscape: UInt16 = 53, kDown: UInt16 = 125, kUp: UInt16 = 126

    public static func intent(for key: KeyPress) -> Intent {
        switch key.keyCode {
        case kReturn, kKeypadEnter: return .answerRecommended
        case kEscape: return .escape
        case kDown: return .next
        case kUp: return .previous
        default: break
        }
        switch (key.chars.lowercased(), key.command) {
        case ("1", false): return .answer(1)
        case ("2", false): return .answer(2)
        case ("3", false): return .answer(3)
        case ("\r", false), ("\n", false): return .answerRecommended
        case ("\u{1b}", false): return .escape
        case ("j", false): return .next
        case ("k", false): return .previous
        case ("s", false): return .snooze
        case ("e", false): return .clear
        case ("d", false): return .openDiff
        case ("o", false): return .openBoard
        case ("o", true): return .openSession
        case ("r", false): return .reply
        case ("l", true): return .askBar
        default: return .none
        }
    }
}

/// `theuth://` links the main window handles (W1S); until then they are inert by design.
public enum DeepLink {
    public static func attention(_ id: String) -> URL {
        // Item ids carry `:` (`decision:<task>:<id>`), which is legal in a path; only spaces and the like are escaped.
        let allowed = CharacterSet.urlPathAllowed.union(CharacterSet(charactersIn: ":"))
        return URL(string: "theuth://attention/\(id.addingPercentEncoding(withAllowedCharacters: allowed) ?? id)")!
    }
    public static func session(_ id: String) -> URL { URL(string: "theuth://c/\(id)")! }
    public static let new = URL(string: "theuth://new")!
}

/// E5: an answer goes out only if the item is still in the latest list.
public enum StaleGuard {
    public static func allows(id: String, latest: [AttentionItem]) -> Bool {
        latest.contains { $0.id == id }
    }
}
