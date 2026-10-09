import AppKit
import CompanionCore
import SwiftUI

/// The recorder's verdict shown under a row: a collision ("Already used by …" + Use here instead), a reserved
/// chord, or a chord another app holds.
struct RowNotice: Equatable {
    var text: String
    /// "Use here instead" moves this chord to the row.
    var moveChord: Chord?
}

/// Our own recorder (W6 item 1b): KeyboardShortcuts' `Recorder`/`RecorderCocoa` fatalErrors in the packaged
/// app because the SwiftPM `Bundle.module` accessor cannot find its resource bundle. Click → first responder
/// with "Type a shortcut…"; a key-down records (global rows need ≥ 1 modifier or an F-key, the collision
/// policy is `KeyTable.check`); ⌫/⌦ clears; ⎋ cancels; clicking away cancels.
final class ShortcutRecorderView: NSView {
    let action: KeyAction
    let model: SettingsModel
    private(set) var recording = false {
        didSet { needsDisplay = true }
    }

    init(action: KeyAction, model: SettingsModel) {
        self.action = action
        self.model = model
        super.init(frame: NSRect(x: 0, y: 0, width: 150, height: 24))
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.cornerCurve = .continuous
        layer?.borderWidth = 1
        setAccessibilityRole(.button)
        setAccessibilityLabel("Shortcut for \(model.keys.binding(action).label)")
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: NSSize { NSSize(width: 150, height: 24) }
    /// Only a click arms the recorder: the window must not hand it first responder on its own (initial key view).
    private var clicked = false
    override var acceptsFirstResponder: Bool { clicked || recording }
    override var focusRingType: NSFocusRingType { get { .none } set {} }

    override func mouseDown(with event: NSEvent) {
        if recording { window?.makeFirstResponder(nil); return }
        clicked = true
        window?.makeFirstResponder(self)
        clicked = false
    }

    override func becomeFirstResponder() -> Bool {
        recording = true
        return true
    }

    override func resignFirstResponder() -> Bool {
        recording = false
        return true
    }

    /// ⌘-chords reach the key equivalent chain before `keyDown`; take them while recording.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard recording, event.type == .keyDown else { return false }
        keyDown(with: event)
        return true
    }

    override func keyDown(with event: NSEvent) {
        guard recording else { return super.keyDown(with: event) }
        let mods = Hotkeys.modifiers(event.modifierFlags)
        let chars = event.charactersIgnoringModifiers ?? ""
        if mods.isEmpty {
            switch Int(event.keyCode) {
            case 53: return end() // ⎋ cancels
            case 51, 117: model.keys.set(nil, for: action); model.rowNotice[action] = nil; return end() // ⌫ ⌦ clear
            case 48: end(); return super.keyDown(with: event) // ⇥ moves on
            default: break
            }
        }
        record(Chord(chars: chars, keyCode: event.keyCode, modifiers: mods))
        end()
    }

    /// The recorded chord through the collision policy; internal so `--probe-settings keys` can drive it without key events.
    func record(_ chord: Chord) {
        let scope = KeyTable.scope(of: action)
        let table = model.keys
        switch table.check(chord, for: action, reserved: scope == .anywhere ? Hotkeys.reserved : []) {
        case .unchanged:
            model.rowNotice[action] = nil
        case .reservedByMacOS:
            NSSound.beep()
            model.rowNotice[action] = RowNotice(text: "\(chord.symbol) is reserved by macOS")
        case .usedBy(let other):
            NSSound.beep()
            model.rowNotice[action] = RowNotice(text: "Already used by \(table.binding(other).label)", moveChord: chord)
        case .ok:
            if scope == .anywhere, !Hotkeys.canRegister(chord) {
                NSSound.beep()
                model.rowNotice[action] = RowNotice(text: "\(chord.symbol) is held by another app — pick another")
                return
            }
            model.keys.set(chord, for: action)
            model.rowNotice[action] = nil
        }
    }

    private func end() {
        if window?.firstResponder === self { window?.makeFirstResponder(nil) }
        recording = false
    }

    override func draw(_ dirtyRect: NSRect) {
        let chord = model.keys.chord(for: action)
        layer?.borderColor = (recording ? NSColor.controlAccentColor : NSColor.separatorColor).cgColor
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor
        let text: String
        let color: NSColor
        if recording { text = "Type a shortcut…"; color = .secondaryLabelColor }
        else if let chord { text = chord.symbol; color = .labelColor }
        else { text = "unassigned"; color = model.keys.binding(action).defaultChord == nil ? .secondaryLabelColor : .systemRed } // lost to a move: warning colour
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12, weight: .medium), .foregroundColor: color]
        let s = NSAttributedString(string: text, attributes: attrs)
        let size = s.size()
        s.draw(at: NSPoint(x: (bounds.width - size.width) / 2, y: (bounds.height - size.height) / 2))
    }
}

/// SwiftUI wrapper: redraws when the table or the recording state changes.
struct ShortcutRecorder: NSViewRepresentable {
    let action: KeyAction
    @ObservedObject var model: SettingsModel

    func makeNSView(context: Context) -> ShortcutRecorderView { ShortcutRecorderView(action: action, model: model) }
    func updateNSView(_ v: ShortcutRecorderView, context: Context) { v.needsDisplay = true }
}

/// One Settings › Keys row: label · recorder (or the fixed chord) · reset, with the verdict line under it.
struct KeyRow: View {
    let binding: KeyBinding
    @ObservedObject var model: SettingsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                Text(binding.label)
                Spacer()
                if binding.fixed {
                    Text(binding.chord?.symbol ?? "").font(.callout.weight(.medium)).frame(width: 150)
                        .foregroundStyle(.secondary).help("Fixed: ⎋ cancels the recorder and closes every surface")
                } else {
                    ShortcutRecorder(action: binding.action, model: model).frame(width: 150, height: 24)
                }
                Button { model.keys.reset(binding.action); model.rowNotice[binding.action] = nil } label: {
                    Image(systemName: "arrow.counterclockwise")
                }
                .buttonStyle(.borderless).disabled(binding.isDefault || binding.fixed)
                .help("Reset to \(binding.defaultChord?.symbol ?? "unassigned")")
                .accessibilityLabel("Reset \(binding.label)")
            }
            if let n = model.rowNotice[binding.action] {
                HStack(spacing: 8) {
                    Text(n.text).font(.caption).foregroundStyle(.orange)
                    if let c = n.moveChord {
                        Button("Use here instead") { model.keys.move(c, to: binding.action); model.rowNotice[binding.action] = nil }
                            .controlSize(.small)
                    }
                }
            }
        }
    }
}
