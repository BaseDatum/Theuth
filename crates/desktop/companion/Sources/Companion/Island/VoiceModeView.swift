import AppKit
import CompanionCore
import SwiftUI

/// 149 §3.7: what the island shows while a gesture is live (96 §0.3's reuse rule) — the rec dot, the live words (finals in
/// ink, the open partial in sand), "· ⌃S held · theuth's desk", the lock, the hint. W3 owns the island window and hosts
/// this view there; until it merges `VoicePill` hosts it in a small capsule near the pointer (mock round 2, screen 7).
@MainActor
final class VoiceModeState: ObservableObject {
    enum Phase: Equatable { case listening, locked, sending, hint }
    @Published var phase = Phase.listening
    @Published var text = ""
    @Published var partial = ""
    @Published var level: Float = 0
    @Published var hint = ""
    @Published var chord = "⌃S"

    func reset(chord: String) {
        phase = .listening; text = ""; partial = ""; level = 0; hint = ""; self.chord = chord
    }
}

struct VoiceModeView: View {
    @ObservedObject var state: VoiceModeState

    var body: some View {
        HStack(spacing: 10) {
            if state.phase == .hint {
                Image(systemName: "info.circle").font(.system(size: 13)).foregroundColor(.sand)
                Text(state.hint).font(.system(size: 14)).foregroundColor(.ink).lineLimit(2)
            } else {
                Circle().fill(Color.carnelian).frame(width: 10, height: 10)
                    .scaleEffect(1 + CGFloat(min(1, state.level)) * 0.6)
                    .animation(.linear(duration: 0.1), value: state.level)
                    .shadow(color: .carnelian.opacity(0.3), radius: 3)
                words
                Text(trailer).font(.system(size: 13)).foregroundColor(.sand).lineLimit(1)
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
        .frame(maxWidth: 560, alignment: .leading)
        .fixedSize(horizontal: true, vertical: true)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(state.phase == .hint ? state.hint : "Listening: \(state.text) \(state.partial)")
    }

    private var words: some View {
        let empty = state.text.isEmpty && state.partial.isEmpty
        return (Text(empty ? "Listening…" : "“\(state.text)").foregroundColor(empty || state.text.isEmpty ? .sand : .ink)
            + Text(state.text.isEmpty || state.partial.isEmpty ? "" : " ")
            + Text(state.partial).foregroundColor(.sand)
            + Text(empty ? "" : "”").foregroundColor(.ink))
            .font(.system(size: 15)).lineLimit(2)
    }

    private var trailer: String {
        switch state.phase {
        case .listening: return "· \(state.chord) held · theuth's desk"
        case .locked: return "· locked · tap \(state.chord) to send"
        case .sending: return "· sending…"
        case .hint: return ""
        }
    }
}

/// The capsule that hosts `VoiceModeView` near the pointer: borderless, non-activating, mouse-transparent, `.popUpMenu`
/// (above the overlays), dark glass (§3.9). Follows the pointer while a gesture is live; a hint lingers `hintMs`.
@MainActor
final class VoicePill {
    /// 96's `HINT_MS`.
    static let hintMs: TimeInterval = 2
    let state = VoiceModeState()
    let panel: NSPanel
    private let hosting: NSHostingView<VoiceModeView>
    private var hintTimer: Timer?

    init() {
        panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 320, height: 40), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = .popUpMenu
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        panel.sharingType = Flags.shareable ? .readOnly : .none
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.ignoresMouseEvents = true
        panel.animationBehavior = .none
        panel.appearance = NSAppearance(named: .darkAqua)
        let container = NSView(frame: panel.contentRect(forFrameRect: panel.frame))
        container.wantsLayer = true
        container.layer?.cornerRadius = 20
        container.layer?.cornerCurve = .continuous
        container.layer?.masksToBounds = true
        container.layer?.backgroundColor = NSColor(hex: 0x12100e, alpha: 0.91).cgColor
        container.layer?.borderWidth = 0.5
        container.layer?.borderColor = NSColor.white.withAlphaComponent(0.08).cgColor
        hosting = NSHostingView(rootView: VoiceModeView(state: state))
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        panel.contentView = container
    }

    var isVisible: Bool { panel.isVisible }

    /// Below-right of `mouse` (AppKit coordinates), kept inside that screen's visible frame.
    func show(near mouse: NSPoint) {
        hintTimer?.invalidate(); hintTimer = nil
        place(near: mouse)
        panel.orderFrontRegardless()
    }

    func follow(_ mouse: NSPoint) { if panel.isVisible { place(near: mouse) } }

    /// A refusal or a hint in the pill's slot for `hintMs`, then hidden.
    func showHint(_ text: String, near mouse: NSPoint) {
        state.phase = .hint
        state.hint = text
        show(near: mouse)
        hintTimer = Timer.scheduledTimer(withTimeInterval: Self.hintMs, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.hide() }
        }
    }

    func hide() {
        hintTimer?.invalidate(); hintTimer = nil
        panel.orderOut(nil)
    }

    private func place(near mouse: NSPoint) {
        hosting.layoutSubtreeIfNeeded()
        let size = hosting.fittingSize
        let screen = NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main
        let v = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        var p = NSPoint(x: mouse.x + 18, y: mouse.y - 26 - size.height)
        p.x = min(max(v.minX + 8, p.x), v.maxX - size.width - 8)
        p.y = min(max(v.minY + 8, p.y), v.maxY - size.height - 8)
        panel.setFrame(NSRect(origin: p, size: size), display: true)
        hosting.frame = panel.contentView?.bounds ?? .zero
    }
}
