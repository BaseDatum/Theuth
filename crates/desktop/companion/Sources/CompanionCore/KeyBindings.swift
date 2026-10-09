import Foundation

/// One key combination, AppKit-free. Printable keys match by the typed character (layout-independent, what the
/// user sees on the cap); everything else (⏎ ⎋ Space arrows F-keys) by the Carbon virtual key code, which is also
/// what a global registration needs. Local-scope chords are stored lower-cased without ⇧ (`normalizedLocal`).
public struct Chord: Hashable, Codable, Sendable, CustomStringConvertible {
    public struct Modifiers: OptionSet, Hashable, Codable, Sendable {
        public let rawValue: UInt8
        public init(rawValue: UInt8) { self.rawValue = rawValue }
        public static let control = Modifiers(rawValue: 1)
        public static let option = Modifiers(rawValue: 2)
        public static let shift = Modifiers(rawValue: 4)
        public static let command = Modifiers(rawValue: 8)
        public static let function = Modifiers(rawValue: 16)
    }

    /// The typed character(s) ignoring modifiers (AppKit's `charactersIgnoringModifiers`); "" for a special key.
    public var chars: String
    /// Carbon virtual key code (HIToolbox).
    public var keyCode: UInt16
    public var modifiers: Modifiers

    public init(chars: String = "", keyCode: UInt16 = 0, modifiers: Modifiers = []) {
        self.chars = chars; self.keyCode = keyCode; self.modifiers = modifiers
    }

    /// A key that types a visible character: letters, digits, punctuation, symbols. Space and the private-use
    /// function characters (`NSUpArrowFunctionKey`…) are not.
    public var isCharKey: Bool {
        guard chars.count == 1, let c = chars.first else { return false }
        return c.isLetter || c.isNumber || c.isPunctuation || c.isSymbol
    }

    public var isFunctionKey: Bool { KeyNames.functionKeys[keyCode] != nil }

    /// Panel/Ask-bar chords: case and ⇧ do not distinguish keys (W1's `K` is `previous`).
    public var normalizedLocal: Chord {
        guard isCharKey else { return self }
        return Chord(chars: chars.lowercased(), keyCode: keyCode, modifiers: modifiers.subtracting(.shift))
    }

    public static func == (a: Chord, b: Chord) -> Bool {
        guard a.modifiers == b.modifiers else { return false }
        return a.isCharKey || b.isCharKey ? a.chars == b.chars : a.keyCode == b.keyCode
    }

    public func hash(into h: inout Hasher) {
        h.combine(modifiers)
        if isCharKey { h.combine(chars) } else { h.combine(keyCode) }
    }

    /// "⌃⇧Space" · "⌘O" · "J" · "⏎": modifiers in Apple's order, then the key's cap.
    public var symbol: String {
        var s = ""
        if modifiers.contains(.control) { s += "⌃" }
        if modifiers.contains(.option) { s += "⌥" }
        if modifiers.contains(.shift) { s += "⇧" }
        if modifiers.contains(.command) { s += "⌘" }
        if modifiers.contains(.function) { s += "fn" }
        return s + KeyNames.name(keyCode: keyCode, chars: chars)
    }

    public var description: String { symbol }

    // Carbon key codes the table and the aliases use.
    public static let returnKey = Chord(chars: "\r", keyCode: 36), keypadEnter = Chord(chars: "\u{3}", keyCode: 76)
    public static let escape = Chord(chars: "\u{1b}", keyCode: 53), space = Chord(chars: " ", keyCode: 49)
    public static let down = Chord(chars: "\u{F701}", keyCode: 125), up = Chord(chars: "\u{F700}", keyCode: 126)
}

/// Key caps for the special keys; letters and digits are their own cap.
public enum KeyNames {
    public static let functionKeys: [UInt16: String] = [
        122: "F1", 120: "F2", 99: "F3", 118: "F4", 96: "F5", 97: "F6", 98: "F7", 100: "F8", 101: "F9", 109: "F10",
        103: "F11", 111: "F12", 105: "F13", 107: "F14", 113: "F15", 106: "F16", 64: "F17", 79: "F18", 80: "F19", 90: "F20",
    ]
    static let special: [UInt16: String] = [
        36: "⏎", 76: "⌅", 53: "⎋", 49: "Space", 48: "⇥", 51: "⌫", 117: "⌦",
        123: "←", 124: "→", 125: "↓", 126: "↑", 115: "↖", 119: "↘", 116: "⇞", 121: "⇟",
    ]

    public static func name(keyCode: UInt16, chars: String) -> String {
        if let s = special[keyCode] ?? functionKeys[keyCode] { return s }
        if Chord(chars: chars).isCharKey { return chars.uppercased() }
        return chars.isEmpty ? "key \(keyCode)" : chars.unicodeScalars.map { String(format: "U+%04X", $0.value) }.joined()
    }
}

public enum KeyScope: String, CaseIterable, Codable, Sendable {
    /// Global hotkeys (Carbon registration): fire in any app.
    case anywhere
    /// While the Attention panel is key.
    case panel
    /// While the Ask bar is key.
    case askBar
    /// 149: while the agents panel is key.
    case agents
    /// 149: the island takes key only in its request box.
    case island
    /// 149: between the voice key's press and the send.
    case pointAndSay

    public var title: String {
        switch self {
        case .anywhere: return "Anywhere"
        case .panel: return "Attention panel"
        case .askBar: return "Ask bar"
        case .agents: return "Agents panel"
        case .island: return "Island"
        case .pointAndSay: return "Point and say"
        }
    }
}

/// Every action a key can trigger; the raw value is the persisted id.
public enum KeyAction: String, CaseIterable, Codable, Sendable {
    case openPanel, openAskBar
    case answer1, answer2, answer3, answerRecommended, escape, next, previous, snooze, clear
    case openDiff, openBoard, openSession, reply, askBar, legend, settings
    case askSend, askEscape, askOpenSession, askLegend
    // 149 (§8 ★ rows; intents are wired by the owning wave):
    case openAgents, voiceKey, meetingPtt, meetingEye
    case replyFocus, talk, point, stopAgent
    case askPoint
    case agentsNext, agentsPrevious, agentsOpenSession, agentsOpenSessionCmd, agentsTalk, agentsStop, agentsEscape
    case islandMute, islandStop, islandDismiss, islandDeeper, islandSend, islandEscape
    case pointEscape

    /// What the Attention panel / Ask bar does; `nil` for the global rows (the app's hotkey handlers).
    public var intent: Intent? {
        switch self {
        case .openPanel, .openAskBar: return nil
        // 149: global rows are the app's handlers; `replyFocus` is R's alias (Revision 1 row 18); the rest are wired by W1/W2/W3.
        case .openAgents, .voiceKey, .meetingPtt, .meetingEye: return nil
        case .replyFocus: return .reply
        case .talk, .point, .stopAgent, .askPoint: return nil
        case .agentsNext: return .next
        case .agentsPrevious: return .previous
        case .agentsOpenSession, .agentsOpenSessionCmd: return .openSession
        case .agentsTalk, .agentsStop: return nil
        case .agentsEscape, .islandEscape, .pointEscape: return .escape
        case .islandMute, .islandStop, .islandDismiss, .islandDeeper, .islandSend: return nil
        case .answer1: return .answer(1)
        case .answer2: return .answer(2)
        case .answer3: return .answer(3)
        case .answerRecommended: return .answerRecommended
        case .escape, .askEscape: return .escape
        case .next: return .next
        case .previous: return .previous
        case .snooze: return .snooze
        case .clear: return .clear
        case .openDiff: return .openDiff
        case .openBoard: return .openBoard
        case .openSession, .askOpenSession: return .openSession
        case .reply: return .reply
        case .askBar: return .askBar
        case .legend, .askLegend: return .legend
        case .settings: return .settings
        case .askSend: return .send
        }
    }
}

public struct KeyBinding: Equatable, Sendable, Identifiable {
    public var action: KeyAction
    public var scope: KeyScope
    public var label: String
    public var defaultChord: Chord?
    /// The effective chord: the override when there is one, else the default. `nil` = unassigned.
    public var chord: Chord?
    /// ⎋ is how the recorder cancels and how every surface closes: never rebound.
    public var fixed = false

    public var id: KeyAction { action }
    public var isDefault: Bool { chord == defaultChord }
}

/// The one data list behind the panel grammar, the legend and Settings › Keys (W6 item 4). Defaults reproduce
/// W1/W3's behaviour exactly; overrides persist as `[actionId: chord | null]` (null = unassigned).
public struct KeyTable: Equatable, Sendable {
    public typealias Overrides = [KeyAction: Chord?]

    public var overrides: Overrides

    public init(overrides: Overrides = [:]) { self.overrides = overrides }

    private static func row(_ a: KeyAction, _ s: KeyScope, _ label: String, _ chord: Chord?, fixed: Bool = false) -> KeyBinding {
        KeyBinding(action: a, scope: s, label: label, defaultChord: chord, chord: chord, fixed: fixed)
    }
    private static func key(_ c: String, _ code: UInt16, _ m: Chord.Modifiers = []) -> Chord { Chord(chars: c, keyCode: code, modifiers: m) }

    /// The table (130 §4.2 · §4.3 · D6; 149 §8 ★ rows), scope order anywhere · panel · askBar · agents · island · pointAndSay.
    public static let defaults: [KeyBinding] = [
        row(.openPanel, .anywhere, "Open the panel · hold to talk", Chord(chars: " ", keyCode: 49, modifiers: [.control, .shift])),
        row(.openAskBar, .anywhere, "Open the Ask bar", nil),
        row(.answer1, .panel, "Answer with option 1", key("1", 18)),
        row(.answer2, .panel, "Answer with option 2", key("2", 19)),
        row(.answer3, .panel, "Answer with option 3", key("3", 20)),
        row(.answerRecommended, .panel, "Answer with the recommended option", .returnKey),
        row(.next, .panel, "Next item", key("j", 38)),
        row(.previous, .panel, "Previous item", key("k", 40)),
        row(.snooze, .panel, "Snooze 30 m", key("s", 1)),
        row(.clear, .panel, "Clear", key("e", 14)),
        row(.openDiff, .panel, "Open the diff", key("d", 2)),
        row(.openBoard, .panel, "Open the board", key("o", 31)),
        row(.openSession, .panel, "Open the session", key("o", 31, .command)),
        row(.reply, .panel, "Reply in words", key("r", 15)),
        row(.askBar, .panel, "Jump to the Ask bar", key("l", 37, .command)),
        row(.legend, .panel, "Keyboard shortcuts", key("/", 44, .command)),
        row(.settings, .panel, "Settings", key(",", 43, .command)),
        row(.escape, .panel, "Undo the hold, else close", .escape, fixed: true),
        row(.askSend, .askBar, "Send to theuth", .returnKey),
        row(.askOpenSession, .askBar, "Open the desk's session", key("o", 31, .command)),
        row(.askLegend, .askBar, "Keyboard shortcuts", key("/", 44, .command)),
        row(.askEscape, .askBar, "Undo the hold, else close", .escape, fixed: true),
        // 149 §8 ★ — anywhere
        row(.openAgents, .anywhere, "Open the agents panel", key("a", 0, [.control, .shift])),
        row(.voiceKey, .anywhere, "Voice key — point and say", key("s", 1, .control)),
        row(.meetingPtt, .anywhere, "Meeting push-to-talk", key("`", 50, .option)),
        row(.meetingEye, .anywhere, "Meeting eye (hide from capture)", key("e", 14, [.option, .shift])),
        // 149 §8 ★ — panel
        row(.replyFocus, .panel, "Focus the reply field", .space),
        row(.talk, .panel, "Hold to talk into the reply", key("v", 9)),
        row(.point, .panel, "Point — attach the screen", key("p", 35)),
        row(.stopAgent, .panel, "Stop the agent (hold)", key("x", 7)),
        // 149 §8 ★ — askBar
        row(.askPoint, .askBar, "Point — attach the screen", key("p", 35)),
        // 149 §8 ★ — agents
        row(.agentsNext, .agents, "Next row", key("j", 38)),
        row(.agentsPrevious, .agents, "Previous row", key("k", 40)),
        row(.agentsOpenSession, .agents, "Open the session", .returnKey),
        row(.agentsOpenSessionCmd, .agents, "Open the session", key("o", 31, .command)),
        row(.agentsTalk, .agents, "Hold to talk into the row", key("v", 9)),
        row(.agentsStop, .agents, "Stop the agent (hold)", key("x", 7)),
        row(.agentsEscape, .agents, "Close", .escape, fixed: true),
        // 149 §8 ★ — island (key only in the request box)
        row(.islandMute, .island, "Mute (pause)", key("m", 46)),
        row(.islandStop, .island, "Stop the meeting (hold)", key("m", 46, .control)), // §8 ⌃⇧M: local chords drop ⇧ (`normalizedLocal`)
        row(.islandDismiss, .island, "Dismiss the topic", key("e", 14)),
        row(.islandDeeper, .island, "Deeper", key("d", 2)),
        row(.islandSend, .island, "Send the request", .returnKey),
        row(.islandEscape, .island, "Leave the box", .escape, fixed: true),
        // 149 §8 ★ — pointAndSay (registered only between press and send)
        row(.pointEscape, .pointAndSay, "Cancel", .escape, fixed: true),
    ]

    /// 149 §8: the per-platform default table. macOS is `defaults`; the Windows twin mirrors this list and reads
    /// every other row's chord with ⌘ → Ctrl (134 D6). Data only — this app is macOS, but the table is the contract.
    public enum Platform: String, CaseIterable, Sendable { case macOS, windows, linux }

    public static let platformDefaults: [KeyAction: [Platform: Chord]] = [
        .voiceKey: [
            .macOS: key("s", 1, .control),
            .windows: Chord(chars: " ", keyCode: 49, modifiers: [.control, .option]),
            .linux: Chord(chars: " ", keyCode: 49, modifiers: [.control, .option]),
        ],
    ]

    /// The default chord of `action` on `platform`: the per-platform row when there is one, else the table's.
    public static func defaultChord(for action: KeyAction, on platform: Platform) -> Chord? {
        if let p = platformDefaults[action]?[platform] { return p }
        return defaults.first { $0.action == action }?.defaultChord
    }

    /// 149g: while the voice key is held, ⎋ reaches Carbon as the chord's modifiers + ⎋ (⌃⎋ for ⌃S), which the plain-⎋
    /// `pointEscape` row never matches — so a gesture also registers this second ⎋ carrying exactly the chord's modifiers (a
    /// rebound key still works; ⌘ or ⌥ only if the chord has them). `nil` = register nothing extra: a bare chord (plain ⎋
    /// covers it), the system's Force Quit chords (`systemEscapes`), or an enabled symbolic hot key (`reserved`, from
    /// `CopySymbolicHotKeys`; tests pass it).
    public static func escapeDuringHold(of chord: Chord, reserved: [Chord] = []) -> Chord? {
        let m = chord.modifiers
        guard !m.isEmpty, !systemEscapes.contains(m) else { return nil }
        let esc = Chord(chars: Chord.escape.chars, keyCode: Chord.escape.keyCode, modifiers: m)
        return reserved.contains(esc) ? nil : esc
    }

    /// ⌘⌥⎋ opens Force Quit; ⌘⌥⇧⎋ held force-quits the front app. Neither is in `CopySymbolicHotKeys`.
    public static let systemEscapes: [Chord.Modifiers] = [[.command, .option], [.command, .option, .shift]]

    /// Fixed synonyms, never rebound: ↓/↑ walk, keypad ⏎ answers, `?` opens the legend. Shown in the legend.
    public static let aliases: [(chord: Chord, action: KeyAction)] = [
        (.down, .next), (.up, .previous), (.keypadEnter, .answerRecommended), (key("?", 44), .legend), (key("?", 44), .askLegend),
        // 149: the agents panel walks like the Attention panel; keypad ⏎ sends in the island's box.
        (.down, .agentsNext), (.up, .agentsPrevious), (.keypadEnter, .agentsOpenSession), (.keypadEnter, .islandSend),
    ]

    /// Every row with its effective chord, table order.
    public var bindings: [KeyBinding] {
        Self.defaults.map { var b = $0; if let o = overrides[b.action] { b.chord = o }; return b }
    }

    public func bindings(in scope: KeyScope) -> [KeyBinding] { bindings.filter { $0.scope == scope } }

    public func binding(_ action: KeyAction) -> KeyBinding { bindings.first { $0.action == action }! }

    public func chord(for action: KeyAction) -> Chord? { binding(action).chord }

    /// The row holding `chord` in `scope`, if any (the table first, then the fixed aliases).
    public func action(for chord: Chord, in scope: KeyScope) -> KeyAction? {
        let c = scope == .anywhere ? chord : chord.normalizedLocal
        if let b = bindings(in: scope).first(where: { $0.chord == c }) { return b.action }
        return Self.aliases.first { $0.chord == c && Self.scope(of: $0.action) == scope }?.action
    }

    public static func scope(of action: KeyAction) -> KeyScope { defaults.first { $0.action == action }!.scope }

    /// Never two actions on one chord in one scope (across scopes is fine: panel J and Ask-bar J never coexist).
    public var isUniquePerScope: Bool {
        KeyScope.allCases.allSatisfy { s in
            let cs = bindings(in: s).compactMap(\.chord)
            return Set(cs).count == cs.count
        }
    }

    // MARK: the collision policy (brief item 4d)

    public enum Verdict: Equatable, Sendable {
        case ok
        /// The row already has this chord.
        case unchanged
        /// A `CopySymbolicHotKeys()` chord or a bare key with no modifier (global scope only).
        case reservedByMacOS
        /// Same scope, another row: the recorder offers "Use here instead" (`move`).
        case usedBy(KeyAction)
    }

    /// `reserved`: the system's symbolic hot keys (the app reads them; tests stub them).
    public func check(_ chord: Chord, for action: KeyAction, reserved: [Chord] = []) -> Verdict {
        let scope = Self.scope(of: action)
        let c = scope == .anywhere ? chord : chord.normalizedLocal
        if self.chord(for: action) == c { return .unchanged }
        if scope == .anywhere, reserved.contains(c) || (c.modifiers.subtracting(.shift).isEmpty && !c.isFunctionKey) {
            return .reservedByMacOS
        }
        if let other = bindings(in: scope).first(where: { $0.chord == c && $0.action != action }) { return .usedBy(other.action) }
        return .ok
    }

    /// Assign without checks (`nil` = unassigned). Local chords are normalized.
    public mutating func set(_ chord: Chord?, for action: KeyAction) {
        let c = Self.scope(of: action) == .anywhere ? chord : chord?.normalizedLocal
        if c == binding(action).defaultChord { overrides.removeValue(forKey: action) } else { overrides[action] = .some(c) }
    }

    /// "Use here instead": the chord moves to `action`; the row that had it goes unassigned.
    public mutating func move(_ chord: Chord, to action: KeyAction) {
        if case .usedBy(let other) = check(chord, for: action) { set(nil, for: other) }
        set(chord, for: action)
    }

    public mutating func reset(_ action: KeyAction) { overrides.removeValue(forKey: action) }
    public mutating func resetAll() { overrides = [:] }

    // MARK: persistence (`CompanionSettings.keyOverrides`)

    public var json: String {
        let raw = Dictionary(uniqueKeysWithValues: overrides.map { ($0.key.rawValue, $0.value) })
        return (try? JSONEncoder().encode(raw)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
    }

    public init(json: String) {
        let raw = (try? JSONDecoder().decode([String: Chord?].self, from: Data(json.utf8))) ?? [:]
        overrides = [:]
        for (k, v) in raw { if let a = KeyAction(rawValue: k) { overrides[a] = .some(v) } }
    }

    // MARK: another app's global hotkey (brief item 4d, third bullet)

    public struct GlobalResolution: Equatable, Sendable {
        public var chord: Chord?
        /// One line for Settings › Keys and the legend; never a dialog.
        public var notice: String?
    }

    /// At launch: the saved global chord if it registers, else the default with a notice, else none.
    /// `canRegister` is the registrar (Carbon's `RegisterEventHotKey` answering `eventHotKeyExistsErr`; tests stub it).
    public static func resolveGlobal(saved: Chord?, default def: Chord?, canRegister: (Chord) -> Bool) -> GlobalResolution {
        guard let saved else { return GlobalResolution(chord: nil, notice: nil) }
        if canRegister(saved) { return GlobalResolution(chord: saved, notice: nil) }
        if let def, def != saved, canRegister(def) {
            return GlobalResolution(chord: def, notice: "\(saved.symbol) is held by another app — using \(def.symbol)")
        }
        return GlobalResolution(chord: nil, notice: "\(saved.symbol) is held by another app — no hotkey until you pick another")
    }
}
