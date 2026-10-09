import AppKit
import Combine
import CompanionCore
import SwiftUI
import TheuthKit

/// 153 §12: the one answer card's view-model — the pure `CompanionCore.AnswerCardState` plus the follow-up draft and the
/// ⌘L history list. `PointController` and the Ask bar (`Store.showAnswerCard`) both write `s`.
@MainActor
final class AnswerCardModel: ObservableObject {
    @Published var s = AnswerCardState()
    @Published var draft = ""
    /// ⌘L: the last 20 answers (`store.ask.receipts`), nil while the list is closed.
    @Published var history: [CompanionCore.Receipt]?
    /// 149s S4: the speaker glyph is a button; clicking it stops the utterance.
    var onStop: (() -> Void)?
    var onFollowUp: ((String) -> Void)?
    var onApprove: ((Decision) -> Void)?
    var onExpandChanged: ((Bool) -> Void)?

    /// Header label (§10.3 / §12.2).
    var who: String { s.kind == .command ? "Command · theuth desk" : "theuth desk" }
    var viewed: AnswerCardState.Turn? { s.turns.indices.contains(s.current) ? s.turns[s.current] : nil }
    var approvalPending: Bool { s.approval.map { $0.outcome == nil } ?? false }

    func toggleExpanded() {
        s.expanded.toggle()
        history = nil
        onExpandChanged?(s.expanded)
    }

    func submitDraft() {
        let t = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        draft = ""
        onFollowUp?(t)
    }
}

struct AnswerCardView: View {
    @ObservedObject var model: AnswerCardModel
    @FocusState private var fieldFocused: Bool

    private var s: AnswerCardState { model.s }
    private var isCommand: Bool { s.kind == .command }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            if let history = model.history { historyList(history) }
            else if s.expanded { expanded } else { collapsed }
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .frame(width: AnswerCard.width)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(model.who) \(s.status): \(s.text)")
    }

    // MARK: header

    private var header: some View {
        HStack(spacing: 8) {
            Text(model.who).font(.body.weight(.semibold)).foregroundColor(.primary).lineLimit(1).fixedSize()
            if s.expanded, s.turns.count > 1 { dots }
            statusText
            if s.speaking { SpeakingGlyph { model.onStop?() } }
            Spacer()
            if isCommand, !s.expanded { Cap("⌃⌥C") }
            Cap("esc")
        }
    }

    /// `thinking` ticks its elapsed seconds (§12.4: never looks frozen).
    @ViewBuilder private var statusText: some View {
        if s.phase == .thinking, let since = s.thinkingSince {
            TimelineView(.periodic(from: since, by: 1)) { ctx in
                let secs = Int(ctx.date.timeIntervalSince(since))
                Text("· \(s.status.isEmpty ? "thinking" : s.status) · \(secs) s").font(.body).foregroundColor(.tertiary).lineLimit(1)
            }
        } else {
            Text("· \(s.status)").font(.body).foregroundColor(s.failed ? .failed : .tertiary).lineLimit(1).truncationMode(.tail).layoutPriority(0)
        }
    }

    private var dots: some View {
        HStack(spacing: 3) {
            Text("\(s.current + 1) of \(s.turns.count)").font(.caption).foregroundColor(.tertiary)
            ForEach(s.turns.indices, id: \.self) { i in
                Circle().strokeBorder(Color.secondary, lineWidth: 1)
                    .background(Circle().fill(i == s.current ? Color.secondary : .clear))
                    .frame(width: 6, height: 6)
            }
        }
        .fixedSize()
    }

    // MARK: collapsed (§12.2: the receipt's two lines; §10.3 the step rows)

    @ViewBuilder private var collapsed: some View {
        if isCommand, let c = s.command { commandBody(c, compact: true) }
        else {
            if !s.question.isEmpty, s.text.isEmpty { Text("“\(s.question)”").font(.body).foregroundColor(.tertiary).lineLimit(2) }
            if !s.text.isEmpty {
                Text(twoLines(s.text)).font(.body).foregroundColor(.primary).lineLimit(2).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else if !s.failed, s.phase == .thinking { ProgressView().controlSize(.small).padding(.vertical, 2) }
            chips(s.tools, running: s.phase == .thinking)
        }
        approvalBox
        if !model.approvalPending { caps(collapsedCaps) }
    }

    private var collapsedCaps: [(String, String)] {
        if isCommand, s.command?.finished == true {
            return [("⏎", "expand"), ("O", "open session"), ("⎋", "dismiss"), ("⌃⌥C", s.command?.frozen == CommandCard.stoppedCopy ? "hold to continue" : "hold to follow up")]
        }
        if isCommand { return [("⎋", "stop"), ("⌃⌥C", "hold to add"), ("⏎", "expand")] }
        return [("⏎", "expand"), ("O", "open"), ("⎋", "dismiss")]
    }

    // MARK: expanded (§12.2)

    @ViewBuilder private var expanded: some View {
        let turn = model.viewed
        ScrollView(.vertical) {
            VStack(alignment: .leading, spacing: 8) {
                if let q = turn?.question, !q.isEmpty { Text("“\(q)”").font(.body).foregroundColor(.tertiary) }
                if let c = turn?.command { commandBody(c, compact: false) }
                else if let text = turn?.text, !text.isEmpty { MarkdownBlocks(text: text, streaming: s.phase == .thinking && s.current == s.turns.count - 1) }
                else if s.phase == .thinking { ProgressView().controlSize(.small).padding(.vertical, 2) }
                if let tools = turn?.tools, !tools.isEmpty, turn?.command == nil { chips(tools, running: s.phase == .thinking) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(maxHeight: AnswerCard.maxBodyHeight)
        approvalBox
        HStack(spacing: 6) {
            TextField("follow up…", text: $model.draft).textFieldStyle(.plain).focused($fieldFocused)
                .onSubmit { model.submitDraft() }
            Cap("⏎")
        }
        .padding(.horizontal, 8).padding(.vertical, 5)
        .background(RoundedRectangle(cornerRadius: Type.buttonRadius, style: .continuous).strokeBorder(Color.separator))
        if !model.approvalPending {
            caps([(isCommand ? "⌃⌥C" : "⌃S", "hold to follow up"), ("⏎", "or type"), ("J K", "thread"), ("⌘L", "list"), ("O", "open"), ("⎋", "collapse")])
        }
    }

    // MARK: command rows (§10.3)

    @ViewBuilder private func commandBody(_ c: CommandCard, compact: Bool) -> some View {
        if let v = c.verdict {
            verdictLine(v)
            Text("\(c.steps) step\(c.steps == 1 ? "" : "s")  \(c.tools.joined(separator: " · "))").font(.caption).foregroundColor(.tertiary).lineLimit(1)
        } else {
            if compact, !c.request.isEmpty { Text("“\(c.request)”").font(.body).foregroundColor(.tertiary).lineLimit(2) }
            let running = c.rows.firstIndex { $0.ok == nil }
            ForEach(Array(c.rows.enumerated()), id: \.element.id) { i, row in
                let queued = running.map { i > $0 } ?? false
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(row.label(queued: queued)).font(.caption.weight(.medium)).foregroundColor(rowColor(row, queued: queued))
                        .frame(width: 64, alignment: .leading)
                    Text(row.text).font(.callout.monospaced()).foregroundColor(.primary).lineLimit(1).truncationMode(.middle)
                    if row.ok == nil, !queued, !c.finished { Shimmer() }
                }
            }
            if let f = c.frozen { Text(f).font(.body).foregroundColor(f.hasPrefix("✗") ? .failed : .paused).fixedSize(horizontal: false, vertical: true) }
            else if c.rows.isEmpty, s.phase == .thinking { ProgressView().controlSize(.small).padding(.vertical, 2) }
        }
    }

    private func rowColor(_ row: CommandCard.Row, queued: Bool) -> Color {
        if queued { return .tertiary }
        switch row.ok { case .some(true): return .done; case .some(false): return .failed; case nil: return .secondary }
    }

    @ViewBuilder private func verdictLine(_ v: CommandCard.Verdict) -> some View {
        switch v {
        case .ok(let t): Text("✓ \(t)").font(.body).foregroundColor(.primary).fixedSize(horizontal: false, vertical: true)
        case .failed(let t): Text("✗ \(t)").font(.body).foregroundColor(.failed).fixedSize(horizontal: false, vertical: true)
        case .replied(let t): Text(twoLines(t)).font(.body).foregroundColor(.primary).lineLimit(s.expanded ? nil : 2)
        }
    }

    // MARK: approval box (§10.6)

    @ViewBuilder private var approvalBox: some View {
        if let a = s.approval {
            VStack(alignment: .leading, spacing: 6) {
                if let o = a.outcome {
                    Text(Self.outcomeLine(o, a)).font(.body).foregroundColor(o == .allowed ? .done : .failed)
                } else {
                    HStack(spacing: 6) {
                        Text("⚠").foregroundColor(.paused)
                        Text("Run ").font(.body) + Text(a.target).font(.body.monospaced()) + Text(" ?").font(.body)
                        Spacer()
                        Text("\(a.risk) · policy: ask").font(.caption).foregroundColor(.tertiary).lineLimit(1)
                    }
                    HStack(spacing: 10) {
                        Button { model.onApprove?(.allow) } label: { HStack(spacing: 4) { Text("Yes"); Cap("⏎") } }.buttonStyle(.bordered)
                        Button { model.onApprove?(.deny) } label: { HStack(spacing: 4) { Text("No"); Cap("⎋") } }.buttonStyle(.bordered)
                        Text(a.missed ? SpokenApproval.missedCopy : (a.listening ? "or say “yes” / “no” · listening" : "or say “yes” / “no”"))
                            .font(.caption).foregroundColor(.tertiary).lineLimit(1)
                    }
                }
            }
            .padding(10)
            .background(RoundedRectangle(cornerRadius: Type.optionRadius, style: .continuous).fill(Color.paused.opacity(a.outcome == nil ? 0.12 : 0.04)))
        }
    }

    static func outcomeLine(_ o: AnswerCardState.Approval.Outcome, _ a: AnswerCardState.Approval) -> String {
        switch o {
        case .allowed: return "✓ approved — \(a.target)"
        case .denied: return "✗ denied — \(a.target)"
        case .timedOut: return "✗ denied (no answer in time) — \(a.target)"
        }
    }

    // MARK: ⌘L list

    private func historyList(_ rows: [CompanionCore.Receipt]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Last answers").font(.caption.weight(.medium)).foregroundColor(.tertiary)
            if rows.isEmpty { Text("nothing in the last 24 h").font(.body).foregroundColor(.tertiary) }
            ForEach(rows) { r in
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(r.who).font(.caption.weight(.medium)).foregroundColor(.secondary).lineLimit(1).frame(width: 72, alignment: .leading)
                    Text(r.text).font(.callout).foregroundColor(.primary).lineLimit(1).truncationMode(.tail)
                }
            }
            caps([("⌘L", "back"), ("⎋", "collapse")])
        }
    }

    // MARK: bits

    private func chips(_ tools: [String], running: Bool) -> some View {
        Group {
            if !tools.isEmpty {
                HStack(spacing: 4) {
                    ForEach(Array(tools.enumerated()), id: \.offset) { i, t in
                        Text(t).font(Type.mono).foregroundColor(.secondary).lineLimit(1)
                            .padding(.horizontal, 5).padding(.vertical, 1)
                            .background(RoundedRectangle(cornerRadius: Type.capRadius, style: .continuous).fill(Color.primary.opacity(0.08)))
                            .opacity(running && i == tools.count - 1 ? 0.7 : 1)
                    }
                }
            }
        }
    }

    private func caps(_ items: [(String, String)]) -> some View {
        HStack(spacing: 12) {
            ForEach(Array(items.enumerated()), id: \.offset) { _, it in
                HStack(spacing: 4) { Cap(it.0); Text(it.1).font(.subheadline).foregroundColor(.tertiary).lineLimit(1) }
            }
            Spacer()
        }
    }

    /// The receipt's two lines (`AskReply.firstTwoLines`), reused as the collapsed form (§12.2).
    private func twoLines(_ text: String) -> String {
        var r = AskReply(session: "")
        r.text = text
        return r.firstTwoLines
    }
}

/// A key cap in the Ask bar's style.
private struct Cap: View {
    let k: String
    init(_ k: String) { self.k = k }
    var body: some View {
        Text(k).font(.caption.weight(.medium).monospaced()).foregroundColor(.primary).lineLimit(1)
            .padding(.horizontal, 5).padding(.vertical, 1)
            .background(RoundedRectangle(cornerRadius: Type.capRadius, style: .continuous).fill(Color.primary.opacity(0.1)))
            .fixedSize().layoutPriority(1)
    }
}

/// The running row's shimmer (§10.3): a short bar whose opacity breathes.
private struct Shimmer: View {
    @State private var on = false
    var body: some View {
        RoundedRectangle(cornerRadius: 2).fill(Color.secondary.opacity(on ? 0.5 : 0.15)).frame(width: 36, height: 8)
            .onAppear { withAnimation(.easeInOut(duration: 0.7).repeatForever(autoreverses: true)) { on = true } }
    }
}

/// 149s S4: "speaking" on the surface that asked for it — the SF speaker glyph in the accent; a click stops the utterance.
struct SpeakingGlyph: View {
    let stop: () -> Void

    var body: some View {
        Button(action: stop) {
            Image(systemName: "speaker.wave.2.fill").font(.subheadline).foregroundColor(.accentColor)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Speaking")
        .accessibilityHint("Click to stop")
    }
}

/// §12.2 markdown: `AttributedString(markdown:)` for inline marks, a tiny block pass for headings, bullets and fenced code.
struct MarkdownBlocks: View {
    let text: String
    var streaming = false

    enum Block: Equatable { case para(String), heading(String), bullet(String), code(String) }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            let blocks = Self.blocks(text)
            ForEach(Array(blocks.enumerated()), id: \.offset) { i, b in
                switch b {
                case .heading(let t): inline(t).font(.body.weight(.semibold))
                case .para(let t): inline(t).font(.body)
                case .bullet(let t): HStack(alignment: .firstTextBaseline, spacing: 6) { Text("•"); inline(t).font(.body) }
                case .code(let t):
                    ScrollView(.horizontal, showsIndicators: false) {
                        Text(t).font(.callout.monospaced()).foregroundColor(.primary).padding(8)
                    }
                    .background(RoundedRectangle(cornerRadius: Type.buttonRadius, style: .continuous).fill(Color.primary.opacity(0.06)))
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if streaming { Shimmer() }
        }
        .textSelection(.enabled)
    }

    private func inline(_ t: String) -> Text {
        Text((try? AttributedString(markdown: t, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(t))
    }

    /// Lines → blocks: ``` fences, `#` headings, `-`/`*`/`•` bullets, paragraphs joined by blank lines.
    static func blocks(_ text: String) -> [Block] {
        var out: [Block] = []
        var para: [String] = []
        var code: [String]?
        func flush() { if !para.isEmpty { out.append(.para(para.joined(separator: " "))); para = [] } }
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = String(raw)
            if line.trimmingCharacters(in: .whitespaces).hasPrefix("```") {
                if let c = code { out.append(.code(c.joined(separator: "\n"))); code = nil } else { flush(); code = [] }
                continue
            }
            if code != nil { code?.append(line); continue }
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.isEmpty { flush(); continue }
            if t.hasPrefix("#") { flush(); out.append(.heading(String(t.drop { $0 == "#" }).trimmingCharacters(in: .whitespaces))); continue }
            if let m = ["- ", "* ", "• "].first(where: t.hasPrefix) { flush(); out.append(.bullet(String(t.dropFirst(m.count)))); continue }
            para.append(t)
        }
        if let c = code { out.append(.code(c.joined(separator: "\n"))) }
        flush()
        return out
    }
}

/// The glass panel (`GlassPanel.make`, the Ask bar's) that takes key without activating the companion so ⎋, O, ⏎, J/K and
/// ⌘L work as soon as the reply lands; dismissed, it hands key back to the app the user was in (the Ask bar's rule).
/// §12.3: one panel that resizes between the collapsed and the expanded anatomy, in the island area of the display chosen
/// at press (Q9).
@MainActor
final class AnswerCard {
    static let width: CGFloat = 440
    /// Expanded body: ≤ 70 % of the screen's visible height (§12.2), the rest scrolls inside.
    static var maxBodyHeight: CGFloat { (NSScreen.main?.visibleFrame.height ?? 800) * 0.7 - 140 }
    let model = AnswerCardModel()
    let panel: KeyPanel
    private let hosting: NSHostingView<AnswerCardView>
    private var keyMonitor: Any?
    private var previousApp: NSRunningApplication?
    private var subs = Set<AnyCancellable>()
    var onEscape: (() -> Void)?
    var onOpen: (() -> Void)?
    /// ⌘L: who supplies the last answers.
    var historySource: (() -> [CompanionCore.Receipt])?
    var onStop: (() -> Void)? {
        get { model.onStop }
        set { model.onStop = newValue }
    }
    var state: AnswerCardState {
        get { model.s }
        set { model.s = newValue }
    }

    init() {
        let (panel, container) = GlassPanel.make(width: Self.width, height: 120)
        self.panel = panel
        panel.level = .popUpMenu
        hosting = NSHostingView(rootView: AnswerCardView(model: model))
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        model.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in DispatchQueue.main.async { self?.relayout() } }
            .store(in: &subs)
    }

    var isVisible: Bool { panel.isVisible }

    /// In the island area of the display under `mouse` (AppKit coordinates; 149i §P — never at the pointer); key unless `key`
    /// is false (probes).
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
        model.history = nil
        if returnKey, let app = previousApp, app != .current { app.activate() }
        previousApp = nil
    }

    /// §15: ⏎ approve · ⎋ deny while the box is up (they win over expand/dismiss); else ⏎ expand/collapse, ⎋ collapse-or-dismiss,
    /// O open, J/K thread, ⌘L list. The follow-up field keeps its typing (⏎ there submits through `onSubmit`).
    private func handle(_ e: NSEvent) -> Bool {
        let typing = panel.firstResponder is NSTextView
        let mods = e.modifierFlags.intersection([.command, .control, .option, .shift])
        if model.approvalPending {
            if e.keyCode == 36 || e.keyCode == 76 { model.onApprove?(.allow); return true }
            if e.keyCode == 53 { model.onApprove?(.deny); return true }
        }
        if e.keyCode == 53 { // ⎋
            if model.history != nil { model.history = nil; return true }
            if model.s.expanded { model.toggleExpanded(); return true }
            onEscape?(); return true
        }
        if mods == .command, e.charactersIgnoringModifiers?.lowercased() == "l" {
            model.history = model.history == nil ? Array((historySource?() ?? []).prefix(20)) : nil
            if !model.s.expanded, model.history != nil { model.s.expanded = true; model.onExpandChanged?(true) }
            return true
        }
        if typing { return false }
        if e.keyCode == 36 || e.keyCode == 76 { model.toggleExpanded(); return true } // ⏎
        guard mods.subtracting(.shift).isEmpty, let ch = e.charactersIgnoringModifiers?.lowercased() else { return false }
        switch ch {
        case "o": onOpen?(); return true
        case "j": model.s.move(1); return true
        case "k": model.s.move(-1); return true
        default: return false
        }
    }

    private var anchor = NSPoint.zero

    private func relayout() { if panel.isVisible { place() } }

    private func place() {
        hosting.layoutSubtreeIfNeeded()
        let size = NSSize(width: Self.width, height: max(84, hosting.fittingSize.height))
        let frame = IslandArea.frame(size: size, on: anchor)
        if panel.isVisible, abs(frame.height - panel.frame.height) > 40 {
            NSAnimationContext.runAnimationGroup { ctx in ctx.duration = 0.18; panel.animator().setFrame(frame, display: true) }
        } else {
            panel.setFrame(frame, display: true)
        }
        hosting.frame = panel.contentView?.bounds ?? .zero
    }
}
