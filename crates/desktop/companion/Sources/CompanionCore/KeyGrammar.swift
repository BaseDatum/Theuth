import Foundation
import TheuthKit

/// A key event stripped of AppKit: the typed characters, the virtual key code and the modifiers.
public struct KeyPress: Equatable, Sendable {
    public var chars: String
    public var keyCode: UInt16
    public var modifiers: Chord.Modifiers
    public var command: Bool { modifiers.contains(.command) }
    public init(chars: String, keyCode: UInt16 = 0, command: Bool = false) {
        self.init(chars: chars, keyCode: keyCode, modifiers: command ? [.command] : [])
    }
    public init(chars: String, keyCode: UInt16 = 0, modifiers: Chord.Modifiers) {
        self.chars = chars; self.keyCode = keyCode; self.modifiers = modifiers
    }
    public var chord: Chord { Chord(chars: chars, keyCode: keyCode, modifiers: modifiers) }
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
    /// ⌘/ (W6): the shortcuts legend; ⌘,: Settings; ⏎ in the Ask bar: send.
    case legend, settings, send
    case none
}

public enum KeyGrammar {
    public static let snoozeInterval: TimeInterval = 30 * 60

    /// The chord looked up in `table` for `scope` (W6: one data list, 130 R2). While a text field has focus
    /// (the send-back note, the R field) single keys are typed, not actions: only ⎋ passes.
    public static func intent(for key: KeyPress, scope: KeyScope = .panel, table: KeyTable = KeyTable(), textFieldFocused: Bool = false) -> Intent {
        guard let action = table.action(for: key.chord, in: scope), let intent = action.intent else { return .none }
        if textFieldFocused, intent != .escape { return .none }
        return intent
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
    /// The whole app, one quit (130 W7, 163): the shell runs its quit handshake.
    public static let quit = URL(string: "theuth://quit")!
}

/// E5: an answer goes out only if the item is still in the latest list.
public enum StaleGuard {
    public static func allows(id: String, latest: [AttentionItem]) -> Bool {
        latest.contains { $0.id == id }
    }
}
