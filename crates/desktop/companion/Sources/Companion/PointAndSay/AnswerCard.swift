import AppKit
import Combine
import CompanionCore
import SwiftUI

/// 149 §3.7 / D14: the desk's streamed reply in the Ask bar's receipt style — "theuth · working…", tool names as they
/// start, the final text, "Open O · ⎋ dismiss". The spoken reply runs beside it (Revision 1 row 10).
@MainActor
final class AnswerCardState: ObservableObject {
    @Published var question = ""
    /// "working…" · "working · ‹tool›" · "replied" · "couldn't reply — …".
    @Published var status = "working…"
    @Published var text = ""
    @Published var failed = false
    @Published var speaking = false

    func begin(question: String) {
        self.question = question; status = "working…"; text = ""; failed = false; speaking = false
    }
}

struct AnswerCardView: View {
    @ObservedObject var state: AnswerCardState

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("theuth").font(.body.weight(.semibold)).foregroundColor(.primary)
                Text("· \(state.status)").font(.body).foregroundColor(state.failed ? .failed : .tertiary).lineLimit(1)
                if state.speaking {
                    Image(systemName: "speaker.wave.2.fill").font(.subheadline).foregroundColor(.done)
                        .accessibilityLabel("Speaking")
                }
                Spacer()
                Cap("esc")
            }
            if !state.question.isEmpty {
                Text("“\(state.question)”").font(.body).foregroundColor(.tertiary).lineLimit(2)
            }
            if !state.text.isEmpty {
                Text(state.text).font(.body).foregroundColor(.primary).lineLimit(12).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else if !state.failed {
                ProgressView().controlSize(.small).padding(.vertical, 2)
            }
            HStack(spacing: 12) {
                HStack(spacing: 4) { Cap("O"); Text("open the desk").font(.subheadline).foregroundColor(.tertiary) }
                HStack(spacing: 4) { Cap("⎋"); Text("dismiss").font(.subheadline).foregroundColor(.tertiary) }
                Spacer()
            }
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .frame(width: AnswerCard.width)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("theuth \(state.status): \(state.text)")
    }
}

/// A key cap in the Ask bar's style.
private struct Cap: View {
    let k: String
    init(_ k: String) { self.k = k }
    var body: some View {
        Text(k).font(.caption.weight(.medium).monospaced()).foregroundColor(.primary)
            .padding(.horizontal, 5).padding(.vertical, 1)
            .background(RoundedRectangle(cornerRadius: Type.capRadius, style: .continuous).fill(Color.primary.opacity(0.1)))
    }
}

/// The glass panel (`GlassPanel.make`, the Ask bar's) that takes key without activating the companion so ⎋ and O work
/// as soon as the reply lands; dismissed, it hands key back to the app the user was in (the Ask bar's rule).
@MainActor
final class AnswerCard {
    static let width: CGFloat = 440
    let state = AnswerCardState()
    let panel: KeyPanel
    private let hosting: NSHostingView<AnswerCardView>
    private var keyMonitor: Any?
    private var previousApp: NSRunningApplication?
    private var subs = Set<AnyCancellable>()
    var onEscape: (() -> Void)?
    var onOpen: (() -> Void)?

    init() {
        let (panel, container) = GlassPanel.make(width: Self.width, height: 120)
        self.panel = panel
        panel.level = .popUpMenu
        hosting = NSHostingView(rootView: AnswerCardView(state: state))
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        state.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in DispatchQueue.main.async { self?.relayout() } }
            .store(in: &subs)
    }

    var isVisible: Bool { panel.isVisible }

    /// Near the pointer (AppKit coordinates); key unless `key` is false (probes).
    func show(near mouse: NSPoint, key: Bool) {
        anchor = mouse
        place()
        if key {
            previousApp = previousApp ?? NSWorkspace.shared.frontmostApplication
            panel.ignoresMouseEvents = false
            panel.makeKeyAndOrderFront(nil)
            if keyMonitor == nil {
                keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
                    MainActor.assumeIsolated {
                        guard let self, self.panel.isKeyWindow else { return e }
                        return self.handle(e) ? nil : e
                    }
                }
            }
        } else {
            panel.ignoresMouseEvents = true
            panel.orderFrontRegardless()
        }
    }

    func hide(returnKey: Bool) {
        guard panel.isVisible else { return }
        if let m = keyMonitor { NSEvent.removeMonitor(m); keyMonitor = nil }
        panel.orderOut(nil)
        if returnKey, let app = previousApp, app != .current { app.activate() }
        previousApp = nil
    }

    private func handle(_ e: NSEvent) -> Bool {
        if e.keyCode == 53 { onEscape?(); return true } // ⎋ — the pointAndSay scope's fixed row
        if e.charactersIgnoringModifiers?.lowercased() == "o", e.modifierFlags.intersection([.control, .option, .shift]).isEmpty {
            onOpen?(); return true // O and ⌘O, as the Ask bar's "Open"
        }
        return false
    }

    private var anchor = NSPoint.zero

    private func relayout() { if panel.isVisible { place() } }

    private func place() {
        hosting.layoutSubtreeIfNeeded()
        let size = NSSize(width: Self.width, height: max(84, hosting.fittingSize.height))
        let screen = NSScreen.screens.first { $0.frame.contains(anchor) } ?? NSScreen.main
        let v = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        var p = NSPoint(x: anchor.x + 18, y: anchor.y - 26 - size.height)
        p.x = min(max(v.minX + 8, p.x), v.maxX - size.width - 8)
        p.y = min(max(v.minY + 8, p.y), v.maxY - size.height - 8)
        panel.setFrame(NSRect(origin: p, size: size), display: true)
        hosting.frame = panel.contentView?.bounds ?? .zero
    }
}
