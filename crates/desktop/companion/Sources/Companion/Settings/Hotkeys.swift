import AppKit
import Carbon.HIToolbox
import CompanionCore
import KeyboardShortcuts

extension KeyboardShortcuts.Name {
    /// `⌃⇧Space` (D6), rebindable through the table (`KeyAction.openPanel`).
    static let openPanel = Self("openPanel", default: .init(.space, modifiers: [.control, .shift]))
    /// Unbound by default (`KeyAction.openAskBar`).
    static let openAskBar = Self("openAskBar")
    /// 149 W1: `⌃⇧A` toggles the agents panel (`KeyAction.openAgents`).
    static let openAgents = Self("openAgents", default: .init(.a, modifiers: [.control, .shift]))
}

/// The `anywhere` scope: the table's global rows pushed into KeyboardShortcuts (Carbon registration), the
/// macOS reserved list and the "another app holds it" probe (W6 item 4d). The table is the source of truth;
/// KeyboardShortcuts' own UserDefaults entry is rewritten from it at launch and on every change.
@MainActor
enum Hotkeys {
    /// 149: a global row is probed and registered only once it is listed here — W1 adds `.openAgents`, W2 `.voiceKey`,
    /// W3 `.meetingPtt`/`.meetingEye`; an unlisted row's chord is data for Settings › Keys and the Windows twin.
    static let names: [KeyAction: KeyboardShortcuts.Name] = [.openPanel: .openPanel, .openAskBar: .openAskBar,
                                                             .meetingPtt: .meetingPtt, .meetingEye: .meetingEye, // 149 W3
                                                             .openAgents: .openAgents, // 149 W1
    ]

    /// Launch: resolve the saved global chords against the registrar (fallback to the default with a notice),
    /// persist the outcome, then hand every global row to KeyboardShortcuts.
    static func start(_ model: SettingsModel) {
        var table = model.value.keys
        var notices: [String] = []
        for b in table.bindings(in: .anywhere) where names[b.action] != nil {
            let r = KeyTable.resolveGlobal(saved: b.chord, default: b.defaultChord, canRegister: canRegister)
            if r.chord != b.chord { table.set(r.chord, for: b.action) }
            if let n = r.notice { notices.append(n); Log.write("hotkey: \(n)") }
        }
        model.hotkeyNotice = notices.isEmpty ? nil : notices.joined(separator: " · ")
        if table != model.value.keys { model.value.keys = table }
        apply(table)
    }

    static func apply(_ table: KeyTable) {
        for (action, name) in names {
            let want = table.chord(for: action).map(shortcut)
            if KeyboardShortcuts.getShortcut(for: name) != want { KeyboardShortcuts.setShortcut(want, for: name) }
        }
    }

    static func shortcut(_ c: Chord) -> KeyboardShortcuts.Shortcut {
        KeyboardShortcuts.Shortcut(KeyboardShortcuts.Key(rawValue: Int(c.keyCode)), modifiers: flags(c.modifiers))
    }

    static func flags(_ m: Chord.Modifiers) -> NSEvent.ModifierFlags {
        var f = NSEvent.ModifierFlags()
        if m.contains(.control) { f.insert(.control) }
        if m.contains(.option) { f.insert(.option) }
        if m.contains(.shift) { f.insert(.shift) }
        if m.contains(.command) { f.insert(.command) }
        if m.contains(.function) { f.insert(.function) }
        return f
    }

    static func modifiers(_ f: NSEvent.ModifierFlags) -> Chord.Modifiers {
        var m = Chord.Modifiers()
        if f.contains(.control) { m.insert(.control) }
        if f.contains(.option) { m.insert(.option) }
        if f.contains(.shift) { m.insert(.shift) }
        if f.contains(.command) { m.insert(.command) }
        if f.contains(.function) { m.insert(.function) }
        return m
    }

    /// The enabled symbolic hot keys (Spotlight ⌘Space, Mission Control, screenshots…): refused for global rows.
    static var reserved: [Chord] {
        var unmanaged: Unmanaged<CFArray>?
        guard CopySymbolicHotKeys(&unmanaged) == noErr, let list = unmanaged?.takeRetainedValue() as? [[String: Any]] else { return [] }
        return list.compactMap {
            guard ($0[kHISymbolicHotKeyEnabled] as? Bool) == true, let code = $0[kHISymbolicHotKeyCode] as? Int, let mods = $0[kHISymbolicHotKeyModifiers] as? Int else { return nil }
            return Chord(chars: "", keyCode: UInt16(truncatingIfNeeded: code), modifiers: modifiers(NSEvent.ModifierFlags(carbon: mods)))
        }
    }

    /// Another app's global hotkey: an exclusive `RegisterEventHotKey` answers `eventHotKeyExistsErr` (-9878).
    /// The probe registration is released at once; KeyboardShortcuts registers the real one. Any other error
    /// is treated as registrable (the real registration will tell).
    static func canRegister(_ c: Chord) -> Bool {
        var ref: EventHotKeyRef?
        let err = RegisterEventHotKey(UInt32(c.keyCode), UInt32(flags(c.modifiers).carbon),
                                      EventHotKeyID(signature: 0x7468_3330, id: 1), GetEventDispatcherTarget(),
                                      OptionBits(kEventHotKeyExclusive), &ref)
        if let ref { UnregisterEventHotKey(ref) }
        return err != OSStatus(eventHotKeyExistsErr)
    }
}

/// Carbon ⇄ AppKit modifier bits (what `RegisterEventHotKey` and `CopySymbolicHotKeys` speak).
extension NSEvent.ModifierFlags {
    var carbon: Int {
        var v = 0
        if contains(.control) { v |= controlKey }
        if contains(.option) { v |= optionKey }
        if contains(.shift) { v |= shiftKey }
        if contains(.command) { v |= cmdKey }
        if contains(.function) { v |= Int(NX_SECONDARYFNMASK) }
        return v
    }

    init(carbon: Int) {
        self.init()
        if carbon & controlKey != 0 { insert(.control) }
        if carbon & optionKey != 0 { insert(.option) }
        if carbon & shiftKey != 0 { insert(.shift) }
        if carbon & cmdKey != 0 { insert(.command) }
        if carbon & Int(NX_SECONDARYFNMASK) != 0 { insert(.function) }
    }
}
