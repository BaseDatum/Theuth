import AppKit
import Carbon.HIToolbox
import CompanionCore

/// D1's `Pasteboard` over `NSPasteboard.general` (153 §13.3): snapshot = every item × every type → data; write = one item
/// with every representation of `PasteboardItem.types`; restore = clear + the items back.
final class SystemPasteboard: CompanionCore.Pasteboard {
    struct ReadFailed: Error { let type: String }
    let pb = NSPasteboard.general

    var changeCount: Int { pb.changeCount }

    func snapshot() throws -> [PasteboardItem] {
        try (pb.pasteboardItems ?? []).map { item in
            var types: [String: Data] = [:]
            for t in item.types {
                // Promised / lazily provided types with no bytes are skipped, not an error (the restore writes what was there).
                if let d = item.data(forType: t) { types[t.rawValue] = d }
            }
            return PasteboardItem(types)
        }
    }

    func write(_ items: [PasteboardItem]) throws -> Int {
        let count = pb.clearContents()
        guard pb.writeObjects(items.map(Self.item)) else { throw ReadFailed(type: "write") }
        return max(count, pb.changeCount)
    }

    func restore(_ items: [PasteboardItem]) throws {
        pb.clearContents()
        guard items.isEmpty || pb.writeObjects(items.map(Self.item)) else { throw ReadFailed(type: "restore") }
    }

    /// Copy without the markers (§13.5 "Copy puts the shaped text on the clipboard without markers").
    func copyPlain(_ text: String) {
        pb.clearContents()
        pb.setString(text, forType: .string)
    }

    private static func item(_ i: PasteboardItem) -> NSPasteboardItem {
        let out = NSPasteboardItem()
        for (t, d) in i.types { out.setData(d, forType: NSPasteboard.PasteboardType(t)) }
        return out
    }
}

/// §13.3 step 3 and §7.4: keystrokes posted to the captured app's pid (`CGEvent.postToPid`), never to the session — the
/// chord the user holds is not merged into them. Media Play/Pause is the `NX_KEYTYPE_PLAY` system-defined event (§8.6).
enum Keystrokes {
    /// ⌘V.
    static func paste(pid: pid_t) -> Bool { tap(CGKeyCode(kVK_ANSI_V), flags: .maskCommand, pid: pid) }
    /// ⏎.
    static func returnKey(pid: pid_t) -> Bool { tap(CGKeyCode(kVK_Return), flags: [], pid: pid) }
    /// ⌫ × n.
    static func backspace(_ n: Int, pid: pid_t) -> Bool {
        var ok = true
        for _ in 0..<max(0, n) { ok = tap(CGKeyCode(kVK_Delete), flags: [], pid: pid) && ok }
        return ok
    }

    private static func tap(_ key: CGKeyCode, flags: CGEventFlags, pid: pid_t) -> Bool {
        guard let down = CGEvent(keyboardEventSource: nil, virtualKey: key, keyDown: true),
              let up = CGEvent(keyboardEventSource: nil, virtualKey: key, keyDown: false) else { return false }
        down.flags = flags; up.flags = flags
        down.postToPid(pid); up.postToPid(pid)
        return true
    }

    /// `NX_KEYTYPE_PLAY` (IOKit `ev_keymap.h`, 16) down + up through the HID tap (the only route the Now Playing item
    /// listens on). macOS offers no public "is something playing", so the caller toggles at start and again at the end
    /// (§8.6 "may un-pause").
    static let keyTypePlay = 16
    static func mediaPlayPause() {
        for down in [true, false] {
            let flags: UInt = down ? 0xA00 : 0xB00
            let data1 = (keyTypePlay << 16) | Int(flags)
            guard let e = NSEvent.otherEvent(with: .systemDefined, location: .zero, modifierFlags: NSEvent.ModifierFlags(rawValue: flags),
                                             timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
                                             subtype: 8, data1: data1, data2: -1) else { continue }
            e.cgEvent?.post(tap: .cghidEventTap)
        }
    }
}
