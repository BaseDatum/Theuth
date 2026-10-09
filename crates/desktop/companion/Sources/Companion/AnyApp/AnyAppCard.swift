import AppKit
import Combine
import CompanionCore
import SwiftUI

/// 153 §13.5 / §13.6 (mock frame A, right): the any-app failure card and the rewrite panel — one glass `KeyPanel` in the
/// island area of the display chosen at press (Q9), `AnswerCard`'s width and key rules (⎋ dismiss; ⏎ the primary action),
/// its own anatomy: title line, the words, action buttons, the footnote. Stays until acted on (D13).
@MainActor
final class AnyAppCardModel: ObservableObject {
    enum Kind: Equatable { case failure, rewrite }
    enum Action { case copy, tryAgain, openInTheuth, replace }

    @Published var kind = Kind.failure
    /// "Couldn't type into Terminal · paste not landed" / "Rewrite · Mail · 312 chars selected".
    @Published var title = ""
    /// The transcript (failure) or the proposal (rewrite).
    @Published var text = ""
    /// Rewrite: the spoken instruction and the selection excerpt.
    @Published var instruction = ""
    @Published var excerpt = ""
    /// "checked at 150 · 350 · 1000 ms — field unchanged · clipboard restored"; after an action: "copied", "pasted again…".
    @Published var note = ""
    @Published var busy = false
    var onAction: ((Action) -> Void)?
}

struct AnyAppCardView: View {
    @ObservedObject var model: AnyAppCardModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text(model.title).font(.body.weight(.semibold)).foregroundColor(model.kind == .failure ? .failed : .primary).lineLimit(1)
                Spacer()
                Cap("esc")
            }
            if model.kind == .rewrite {
                if !model.instruction.isEmpty { Text("“\(model.instruction)”").font(.body).foregroundColor(.tertiary).lineLimit(2) }
                if !model.excerpt.isEmpty {
                    Text(model.excerpt).font(.body).foregroundColor(.tertiary).lineLimit(2).truncationMode(.tail)
                        .padding(.leading, 8).overlay(alignment: .leading) { Rectangle().fill(Color.separator).frame(width: 2) }
                }
            }
            if model.busy { HStack(spacing: 8) { ProgressView().controlSize(.small); Text(model.kind == .rewrite ? "rewriting…" : "pasting again…").font(.body).foregroundColor(.tertiary) } }
            else if !model.text.isEmpty {
                Text(model.text).font(.body).foregroundColor(.primary).lineLimit(model.kind == .rewrite ? 8 : 4).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack(spacing: 8) {
                switch model.kind {
                case .failure:
                    Button("Copy") { model.onAction?(.copy) }
                    Button("Try again") { model.onAction?(.tryAgain) }
                    Button("Open in theuth") { model.onAction?(.openInTheuth) }
                case .rewrite:
                    Button("Replace") { model.onAction?(.replace) }.disabled(model.busy || model.text.isEmpty)
                    Cap("⏎")
                    Spacer(minLength: 4)
                    Text("Refine: hold \(AnyAppController.chordSymbol) · ⎋ discard").font(.caption).foregroundColor(.tertiary).lineLimit(1)
                }
            }
            .controlSize(.small)
            if !model.note.isEmpty { Text(model.note).font(.caption).foregroundColor(.tertiary).lineLimit(2) }
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .frame(width: AnswerCard.width)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(model.title): \(model.text)")
    }
}

/// A key cap in the Ask bar's style (the card's `Cap`, private there).
private struct Cap: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text).font(Type.mono).foregroundColor(.secondary).padding(.horizontal, 5).padding(.vertical, 1)
            .background(RoundedRectangle(cornerRadius: Type.capRadius).fill(Color.secondary.opacity(0.15)))
    }
}

@MainActor
final class AnyAppCard {
    let model = AnyAppCardModel()
    let panel: KeyPanel
    private let hosting: NSHostingView<AnyAppCardView>
    private var keyMonitor: Any?
    private var subs = Set<AnyCancellable>()
    private var anchor = NSPoint.zero
    var onEscape: (() -> Void)?

    init() {
        let (panel, container) = GlassPanel.make(width: AnswerCard.width, height: 120)
        self.panel = panel
        panel.level = .popUpMenu
        hosting = NSHostingView(rootView: AnyAppCardView(model: model))
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        model.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in DispatchQueue.main.async { if self?.panel.isVisible == true { self?.place() } } }
            .store(in: &subs)
    }

    var isVisible: Bool { panel.isVisible }

    /// In the island area of the display under `mouse`; key unless a probe (`key: false`).
    func show(near mouse: NSPoint, key: Bool) {
        anchor = mouse
        place()
        if key {
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

    /// Hidden without handing key back: the controller restores focus to the captured element itself (§13.6 Replace) or
    /// leaves the user where they are.
    func hide() {
        guard panel.isVisible else { return }
        if let m = keyMonitor { NSEvent.removeMonitor(m); keyMonitor = nil }
        panel.orderOut(nil)
    }

    /// ⎋ dismiss · ⏎ Replace (rewrite) / Try again (failure) · C copy · O open in theuth.
    private func handle(_ e: NSEvent) -> Bool {
        if e.keyCode == 53 { onEscape?(); return true }
        let mods = e.modifierFlags.intersection([.command, .control, .option])
        guard mods.isEmpty else { return false }
        if e.keyCode == 36 || e.keyCode == 76 { model.onAction?(model.kind == .rewrite ? .replace : .tryAgain); return true }
        guard model.kind == .failure, let ch = e.charactersIgnoringModifiers?.lowercased() else { return false }
        switch ch {
        case "c": model.onAction?(.copy); return true
        case "o": model.onAction?(.openInTheuth); return true
        default: return false
        }
    }

    private func place() {
        hosting.layoutSubtreeIfNeeded()
        let size = NSSize(width: AnswerCard.width, height: max(84, hosting.fittingSize.height))
        let frame = IslandArea.frame(size: size, on: anchor)
        if panel.isVisible, abs(frame.height - panel.frame.height) > 40 {
            NSAnimationContext.runAnimationGroup { ctx in ctx.duration = 0.18; panel.animator().setFrame(frame, display: true) }
        } else {
            panel.setFrame(frame, display: true)
        }
        hosting.frame = panel.contentView?.bounds ?? .zero
    }
}
