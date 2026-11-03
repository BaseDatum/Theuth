import AppKit
import Combine
import CompanionCore
import TheuthKit

// MARK: 149 W1 §3.3 — V · P · X on the card (the table rows without an intent)

extension PanelController {
    /// `.talk` (V, hold-to-talk into the field), `.point` (P), `.stopAgent` (X). `true` when the key was consumed.
    /// While a field is up (the note or the reply field) the field types — except V's key-up, which must end a hold.
    func handleW1(_ a: KeyAction, _ e: NSEvent) -> Bool {
        let down = e.type == .keyDown
        let fieldUp = store.noteFor != nil || store.ask.replyFor != nil
        switch a {
        case .talk:
            if !down {
                guard w1.voice.isBusy else { return false }
                w1.voice.release()
                return true
            }
            if e.isARepeat { return w1.voice.isBusy } // the held key's repeats go nowhere
            guard !fieldUp, let c = store.focusedCard, c.replyAction("") != nil else { return false }
            w1.voice.press(target: c.item.id)
            return true
        case .point:
            guard down, !fieldUp else { return false }
            pointAndAttach()
            return true
        case .stopAgent:
            guard down, !fieldUp else { return false }
            stopAgent()
            return true
        default:
            return false
        }
    }

    /// X / the "Stop agent X" pill (D3, Revision 1 row 2): only when the anatomy offers `.stop`; `cancel(session, tree)`
    /// after the hold, the pill reads "Stopping ‹who› and N children".
    func stopAgent() {
        guard !AskFlags.automated, let c = store.focusedCard, let s = c.item.session,
              store.anatomy(for: c).footer.contains(.stop) else { return }
        let children = Int(store.activity.first { $0.id == s }?.activity.children ?? 0)
        store.hold(Store.Held(itemId: c.item.id, who: c.who, label: "stopping", action: .stopAgent(session: s, children: children)))
    }

    /// P (§3.3): close, read the screen under the pointer once (`screen_context`, a one-point path), reopen with the
    /// attachment on the card and the field focused. A failure shows "Failed · screen: …" and reopens without one.
    func pointAndAttach() {
        guard !AskFlags.automated, !w1.attaching, let c = store.focusedCard, c.item.session != nil else { return }
        w1.attaching = true
        close(returnKey: false)
        // One run-loop turn so the panel is off screen before the engine shoots it.
        DispatchQueue.main.async { [self] in
            let m = NSEvent.mouseLocation
            let top = NSScreen.screens.first?.frame.maxY ?? 0 // CG: top-left origin
            let req = ScreenContextReq(samples: [PointerSample(tMs: 0, x: Float(m.x), y: Float(top - m.y), pen: true)],
                                       color: PointController.accentHex, sinceMs: UInt64(Date().timeIntervalSince1970 * 1000),
                                       accessibility: true)
            Task { @MainActor [self] in
                do {
                    self.w1.attached = try await self.store.client.screenContext(req)
                } catch {
                    self.store.setReceipt(.failed("screen: \(error)"))
                    self.store.expireReceipt(after: 6)
                }
                self.w1.attaching = false
                self.open(anchor: .mouse)
                self.store.ask.replyFor = c.item.id
            }
        }
    }
}

extension Store {
    /// 149 W1 (§3.3 P): the reply with the screen attached — `[screen] …` lines appended, the PNGs as `files` →
    /// `send_input(session, text, files)` after the hold. Empty text with no lines is refused like `submitReply`.
    func submitReplyWithScreen(_ text: String, attached: ScreenContextReply) {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let c = focusedCard, let s = c.item.session, !(t.isEmpty && attached.lines.isEmpty) else { return }
        let body = ((t.isEmpty ? [] : [t]) + attached.lines.map { "[screen] \($0)" }).joined(separator: "\n")
        ask.replyFor = nil
        hold(Held(itemId: c.item.id, who: c.who, label: "replied · screen attached",
                  action: .replyWithScreen(session: s, text: body, files: attached.files)))
    }
}

// MARK: 149 W1 §10 — `--probe-panel <finished|ask|approval|review|empty>`

/// Seeds one card (plus two needs-you lines so the position dots show three), shows the panel not key at t+1.5 s, logs
/// its frame and footer verbs for the shell's `screencapture -R`, quits at t+6 s. Nothing is sent, no microphone.
enum PanelProbe {
    @MainActor private static var subs = Set<AnyCancellable>()

    @MainActor static func run(kind: String, store: Store, attention: PanelController) {
        let f = fixture(kind)
        func seed() {
            store.w1Seed(items: f.items, agents: [], activity: f.activity, sessions: f.sessions, mesh: nil, status: nil, packets: f.packets)
            if let id = f.focus { store.focus(id: id) }
        }
        seed()
        // A reachable engine's refetch or a reconnect must never overwrite the fixture.
        store.onRefetched = { seed() }
        store.$connection.removeDuplicates().receive(on: RunLoop.main)
            .sink { _ in MainActor.assumeIsolated { seed() } }.store(in: &subs)
        W1Log.write("probe-panel start kind=\(kind) items=\(store.items.count) focused=\(store.panel.focused?.id ?? "nil") shareable=\(Flags.shareable) bundle=\(Bundle.main.bundleIdentifier ?? "nil")")

        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
            attention.showWithoutKey(anchor: .mouse)
            let anatomy = store.focusedCard.map { store.anatomy(for: $0) }
            W1Log.write("probe-panel kind=\(kind) frame=\(attention.panel.frame) visible=\(attention.panel.isVisible) key=\(attention.panel.isKeyWindow) active=\(NSApp.isActive) position=\"\(store.panel.position)\" headline=\"\(store.panel.headline)\" footer=\(anatomy.map { "\($0.footer)" } ?? "none") status=\(anatomy.map { "\($0.status.state) · \($0.status.age) · \($0.status.project ?? "-")" } ?? "none") bubbles=\(anatomy?.bubbles.count ?? 0) options=\(anatomy?.options.count ?? 0)")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 6) {
            // Ground truth from the window server (top-left origin), as `AppDelegate.runProbe` reads it.
            let wins = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
                .filter { ($0[kCGWindowOwnerPID as String] as? Int32) == getpid() }
                .compactMap { $0[kCGWindowBounds as String] as? [String: Any] }
                .map { "x=\($0["X"] ?? 0) y=\($0["Y"] ?? 0) w=\($0["Width"] ?? 0) h=\($0["Height"] ?? 0)" }
            W1Log.write("probe-panel windows (CGWindowList): \(wins)")
            W1Log.write("probe-panel done kind=\(kind) — quitting (frame=\(attention.panel.frame) key=\(attention.panel.isKeyWindow) active=\(NSApp.isActive))")
            NSApp.terminate(nil)
        }
    }

    private struct Fixture {
        var items: [AttentionItem] = []
        var activity: [AgentInfo] = []
        var sessions: [SessionSummary] = []
        var packets: [String: Packet] = [:]
        var focus: String?
    }

    private static func fixture(_ kind: String) -> Fixture {
        let now = UInt64(Date().timeIntervalSince1970 * 1000)
        var f = Fixture()
        switch kind {
        case "finished":
            f.items = [AttentionItem(id: "fin-3", kind: .finished, title: "Island panel is in; two next steps", atMs: now - 20_000,
                                     agent: "w3-149-island", session: "s-3",
                                     summary: "Island panel is in. Drag, snap-back and per-display position memory; 14 tests green.",
                                     nextSteps: ["Looks good — merge it", "Show me the island on a screen without a notch"])]
            f.activity = [AgentInfo(id: "s-3", name: "w3-149-island", agent: "omp", state: .idle, last: "14 tests green", project: "theuth",
                                    activity: SessionActivity(state: .idle))]
            f.sessions = [SessionSummary(id: "s-3", title: "Bring the island native", project: "theuth")]
            f.focus = "fin-3"
        case "ask":
            let q = Question(id: "q", question: "Which spacing between the rail pills?",
                             options: [Choice(label: "12 pt gaps, 52 pt pills"), Choice(label: "8 pt gaps, 44 pt pills"),
                                       Choice(label: "One continuous capsule as today")], recommended: 0)
            f.items = [AttentionItem(id: "ask-1", kind: .ask, title: q.question, atMs: now - 120_000, agent: "w1-149-parity", session: "s-1",
                                     ask: AskRequested(id: "ask-1", session: "s-1", questions: [q]),
                                     context: ItemContext(request: "Bring the rail to One's pill layout",
                                                          lastReply: "Three layouts are drafted. One question before I build."))]
            f.activity = [AgentInfo(id: "s-1", name: "w1-149-parity", agent: "omp", state: .awaiting(from: "ask"), last: "One question before I build.",
                                    project: "theuth", activity: SessionActivity(state: .awaiting(from: "ask"), asks: 1, live: true))]
            f.sessions = [SessionSummary(id: "s-1", title: "Bring the rail to One's pill layout", project: "theuth", running: true)]
            f.focus = "ask-1"
        case "approval":
            f.items = [AttentionItem(id: "ap-1-item", kind: .approval, title: "Run bash: cargo test -p computer", atMs: now - 30_000,
                                     agent: "w2-149-point", session: "s-2", approval: "ap-1",
                                     context: ItemContext(request: "Stroke the point-and-say overlay natively",
                                                          lastReply: "Overlay strokes; running the computer tests before I report."))]
            f.activity = [AgentInfo(id: "s-2", name: "w2-149-point", agent: "omp", state: .approval(tool: "bash"), last: "cargo test -p computer",
                                    project: "theuth", activity: SessionActivity(state: .approval(tool: "bash"), children: 2, live: true))]
            f.sessions = [SessionSummary(id: "s-2", title: "Point-and-say native", project: "theuth", running: true)]
            f.focus = "ap-1-item"
        case "review":
            f.items = [AttentionItem(id: "rev-7", kind: .review, title: "Point-and-say overlays stroke natively", atMs: now - 300_000,
                                     task: "t-7", agent: "w2-149-overlay", session: "s-4", packet: "pk-7")]
            f.packets = ["pk-7": Packet(key: "pk-7", task: "t-7", kind: .review, by: "w2-149-overlay", atMs: now - 300_000,
                                        summary: "Point-and-say overlays stroke natively",
                                        options: [PacketOption(label: "Merge"), PacketOption(label: "Hold for Sky")],
                                        checks: [Check(text: "17 tests", done: true), Check(text: "probe PNG", done: false)],
                                        ifWrong: "A wrong merge ships a broken overlay to the morning build")]
            f.activity = [AgentInfo(id: "s-4", name: "w2-149-overlay", agent: "omp", state: .running, last: "rendering the probe PNG",
                                    project: "theuth", activity: SessionActivity(state: .running, live: true))]
            f.sessions = [SessionSummary(id: "s-4", title: "Overlay strokes", project: "theuth", running: true)]
            f.focus = "rev-7"
        case "empty":
            break
        default:
            W1Log.write("probe-panel unknown kind \(kind) — showing the empty state")
        }
        if !f.items.isEmpty {
            // Two younger needs-you lines: the dots read ● ● ▬ / ▬ ● ●.
            f.items += [
                AttentionItem(id: "x-ap", kind: .approval, title: "Run cargo fmt --check", atMs: now - 10_000, agent: "w0-149-engine",
                              session: "s-9", approval: "ap-9"),
                AttentionItem(id: "x-dec", kind: .decision, title: "Ship the rail behind a flag?", atMs: now - 5_000, agent: "theuth",
                              session: "s-8", decision: "d-8"),
            ]
        }
        return f
    }
}
