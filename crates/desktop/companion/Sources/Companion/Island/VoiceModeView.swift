import AppKit
import Combine
import CompanionCore
import SwiftUI

/// 149 §3.7: what the island shows while a gesture is live (96 §0.3's reuse rule) — the rec dot, the live words (finals
/// primary, the open partial tertiary), "· ⌃S held · theuth's desk", the lock, the hint. W3 owns the island window and hosts
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
                Image(systemName: "info.circle").font(.body).foregroundColor(.tertiary)
                Text(state.hint).font(.body).foregroundColor(.primary).lineLimit(2)
            } else {
                Circle().fill(Color.accentColor).frame(width: 10, height: 10)
                    .scaleEffect(1 + CGFloat(min(1, state.level)) * 0.6)
                    .animation(.linear(duration: 0.1), value: state.level)
                    .shadow(color: .accentColor.opacity(0.3), radius: 3)
                words
                Text(trailer).font(.body).foregroundColor(.tertiary).lineLimit(1)
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
        return (Text(empty ? "Listening…" : "“\(state.text)").foregroundColor(empty || state.text.isEmpty ? .tertiary : .primary)
            + Text(state.text.isEmpty || state.partial.isEmpty ? "" : " ")
            + Text(state.partial).foregroundColor(.tertiary)
            + Text(empty ? "" : "”").foregroundColor(.primary))
            .font(.body).lineLimit(2)
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

/// The capsule that hosts `VoiceModeView`: borderless, non-activating, mouse-transparent, `.popUpMenu` (above the overlays),
/// HUD glass (§3.9). 149i §P: it sits in the island area of the display under the pointer — under the notch, or the fallback
/// slot under the menu bar, below a live island — fixed for the whole gesture; only the trail follows the pointer. A hint
/// lingers `hintMs`.
@MainActor
final class VoicePill {
    /// 96's `HINT_MS`.
    static let hintMs: TimeInterval = 2
    let state = VoiceModeState()
    let panel: NSPanel
    private let hosting: NSHostingView<VoiceModeView>
    private var hintTimer: Timer?
    private var sub: AnyCancellable?
    /// The display the gesture started on (the pointer at `show`); the frame is recomputed there as the text grows, never
    /// at the pointer's current position.
    private var displayPoint = NSPoint.zero

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
        let container = NSVisualEffectView(frame: panel.contentRect(forFrameRect: panel.frame))
        container.material = .hudWindow
        container.blendingMode = .behindWindow
        container.state = .active
        container.wantsLayer = true
        container.layer?.cornerRadius = Type.cardRadius
        container.layer?.cornerCurve = .continuous
        container.layer?.masksToBounds = true
        container.layer?.borderWidth = 0.5
        container.layer?.borderColor = NSColor.separatorColor.cgColor
        hosting = NSHostingView(rootView: VoiceModeView(state: state))
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        panel.contentView = container
        sub = state.objectWillChange.receive(on: RunLoop.main).sink { [weak self] _ in
            DispatchQueue.main.async { guard let self, self.panel.isVisible else { return }; self.place() }
        }
    }

    var isVisible: Bool { panel.isVisible }

    /// In the island area of the display under `mouse` (AppKit coordinates); the pointer never moves it afterwards.
    func show(near mouse: NSPoint) {
        hintTimer?.invalidate(); hintTimer = nil
        displayPoint = mouse
        place()
        panel.orderFrontRegardless()
    }

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

    private func place() {
        hosting.layoutSubtreeIfNeeded()
        let frame = IslandArea.frame(size: hosting.fittingSize, on: displayPoint)
        if panel.frame != frame { panel.setFrame(frame, display: true) }
        hosting.frame = panel.contentView?.bounds ?? .zero
    }
}
