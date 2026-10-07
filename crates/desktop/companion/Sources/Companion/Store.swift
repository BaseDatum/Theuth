import AppKit
import Combine
import CompanionCore
import TheuthKit

/// Engine state the surfaces render, plus the hold queue. Everything here runs on the main actor; the RPC
/// client calls back from its own executor and we hop.
@MainActor
final class Store: ObservableObject {
    struct Held: Equatable {
        let itemId: String
        let who: String
        let label: String
        let action: CardAction
    }

    enum Receipt: Equatable {
        case holding(Held)
        case sent(String)
        case undone
        case stale
        case failed(String)
    }

    @Published private(set) var items: [AttentionItem] = []
    @Published private(set) var agents: [AgentSummary] = []
    @Published private(set) var activity: [AgentInfo] = []
    @Published private(set) var approvals: [String: ApprovalRequested] = [:]
    @Published private(set) var packets: [String: Packet] = [:]
    @Published private(set) var rail: Rail = .empty
    @Published private(set) var panel = PanelModel(items: [])
    @Published private(set) var connection: RpcClient.State = .connecting
    @Published private(set) var receipt: Receipt?
    @Published private(set) var now = Date()
    /// A review "Send back…" is collecting its note for this task.
    @Published var noteFor: String?

    private(set) var undo = UndoQueue<Held>(delay: Settings.undoDelay)
    private var client: RpcClient!
    private var refetchTimer: Timer?
    private var tickTimer: Timer?
    private var receiptTimer: Timer?
    private var fetching = false
    private var fetchAgain = false

    /// Hooks the panel controller sets.
    var onCloseRequest: (() -> Void)?
    var onOpenURL: ((URL) -> Void)?
    /// Probe hook: the first `PendingPrompts`.
    var onPendingPrompts: ((PendingPrompts) -> Void)?
    /// Probe hook: a full refetch landed.
    var onRefetched: (() -> Void)?

    func start(location: EngineLocation) {
        client = RpcClient(location: location,
                           onState: { s in Task { @MainActor in self.connection = s } },
                           onEvent: { d in Task { @MainActor in self.handle(d) } })
        Task {
            await client.start()
            await client.subscribeEvents()
        }
    }

    // MARK: events

    private func handle(_ data: Data) {
        guard let event = try? WireEvent.decode(data) else { return }
        switch event {
        case .pendingPrompts(let p):
            approvals = Dictionary(p.approvals.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
            onPendingPrompts?(p)
            scheduleRefetch(now: true)
        case .approvalRequested(let a):
            approvals[a.id] = a
            scheduleRefetch()
        case .approvalResolved(let r):
            approvals[r.id] = nil
            scheduleRefetch()
        case .askRequested, .askResolved, .attentionChanged, .turnFinished:
            scheduleRefetch()
        case .lagged:
            scheduleRefetch(now: true)
        case .other(let t):
            if WireEvent.attentionTriggers.contains(t) || t == "TurnStarted" || t == "SubagentSpawned" { scheduleRefetch() }
        }
    }

    /// Debounced 250 ms (130 §5.2); never on a timer.
    private func scheduleRefetch(now immediate: Bool = false) {
        refetchTimer?.invalidate()
        if immediate { return refetch() }
        refetchTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.refetch() }
        }
    }

    func refetch() {
        if fetching { fetchAgain = true; return }
        fetching = true
        Task {
            defer { fetching = false }
            do {
                async let a = client.attentionList()
                async let g = client.agentsList()
                async let l = client.listActivity()
                let (items, agents, activity) = try await (a, g, l)
                apply(items: items, agents: agents, activity: activity)
                await loadPackets()
                onRefetched?()
            } catch {
                Log.write("refetch failed: \(error)")
            }
            if fetchAgain { fetchAgain = false; refetch() }
        }
    }

    private func apply(items: [AttentionItem], agents: [AgentSummary], activity: [AgentInfo]) {
        self.items = items
        self.agents = agents
        self.activity = activity
        rail = RailModel.derive(items: items, agents: agents, activity: activity, now: Date())
        panel.workingCount = rail.workingCount
        panel.update(items: items)
    }

    /// `packet_get` for every decision/review in the list that has none cached yet.
    private func loadPackets() async {
        for key in Set(items.compactMap(\.packet)).subtracting(packets.keys) {
            if let p = try? await client.packetGet(key: key) { packets[key] = p }
        }
    }

    // MARK: the card

    func card(for item: AttentionItem) -> Card {
        Card.make(item: item,
                  packet: item.packet.flatMap { packets[$0] },
                  approval: item.approval.flatMap { approvals[$0] },
                  agentModel: agents.first { $0.file.name == item.agent }?.file.model)
    }

    var focusedCard: Card? { panel.focused.map(card(for:)) }

    func focus(id: String) { panel.focus(id: id) }

    // MARK: keys → actions

    func perform(_ intent: Intent) {
        switch intent {
        case .answer(let n):
            guard let c = focusedCard, let o = c.options.first(where: { $0.key == n }) else { return }
            pick(o, card: c)
        case .answerRecommended:
            guard let c = focusedCard, let o = c.recommended else { return } // P13: nothing pre-picked → ⏎ is inert
            pick(o, card: c)
        case .escape:
            if noteFor != nil { noteFor = nil; return }
            if undo.cancel() { receipt = .undone; expireReceipt(after: 1.5); return }
            onCloseRequest?()
        case .next: panel.move(1)
        case .previous: panel.move(-1)
        case .snooze:
            guard let c = focusedCard else { return }
            hold(Held(itemId: c.item.id, who: c.who, label: "snoozed 30 m", action: .snooze(id: c.item.id)))
        case .clear:
            guard let c = focusedCard, c.clearable else { return }
            hold(Held(itemId: c.item.id, who: c.who, label: "cleared", action: .dismiss(id: c.item.id)))
        case .openDiff, .openBoard:
            guard let c = focusedCard else { return }
            onOpenURL?(c.attentionURL)
        case .openSession:
            guard let url = focusedCard?.sessionURL else { return }
            onOpenURL?(url)
        case .none: break
        }
    }

    func pick(_ option: CardOption, card: Card) {
        if option.action.needsNote, case .reviewSendBack(let task) = option.action {
            noteFor = task
            return
        }
        hold(Held(itemId: card.item.id, who: card.who, label: option.label, action: option.action))
    }

    /// The typed send-back note; empty is refused (the note is required).
    func submitNote(_ note: String) {
        let text = note.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let task = noteFor, !text.isEmpty, let c = focusedCard else { return }
        noteFor = nil
        hold(Held(itemId: c.item.id, who: c.who, label: "sent back", action: .reviewSendBackNote(task: task, note: text)))
    }

    private func hold(_ h: Held) {
        undo = UndoQueue(delay: Settings.undoDelay)
        if let displaced = undo.hold(h, now: Date()) { commit(displaced) }
        receipt = .holding(h)
        receiptTimer?.invalidate()
        tickTimer?.invalidate()
        now = Date()
        tickTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.tick() }
        }
    }

    private func tick() {
        now = Date()
        if let fired = undo.tick(now: now) {
            tickTimer?.invalidate()
            tickTimer = nil
            commit(fired)
        } else if undo.held == nil {
            tickTimer?.invalidate()
            tickTimer = nil
        }
    }

    var holdRemaining: TimeInterval { undo.remaining(now: now) }

    /// E5: re-read, then send; the engine's `false` is "already answered elsewhere", never retried.
    private func commit(_ h: Held) {
        Task {
            do {
                let latest = try await client.attentionList()
                guard StaleGuard.allows(id: h.itemId, latest: latest) else {
                    receipt = .stale
                    apply(items: latest, agents: agents, activity: activity)
                    return expireReceipt(after: 3)
                }
                var ok = true
                switch h.action {
                case .answerAsk(let id, let answers): ok = try await client.answerAsk(id: id, answers: answers)
                case .decide(let decision, let answer): try await client.taskDecide(decision: decision, answer: answer)
                case .approve(let id, let decision): ok = try await client.resolveApproval(id: id, decision: decision)
                case .reviewApprove(let task): try await client.taskMove(id: task, state: "done", note: nil)
                case .reviewSendBackNote(let task, let note): try await client.taskMove(id: task, state: "todo", note: note)
                case .reviewSendBack: return
                case .sendNextStep(let session, let text):
                    _ = try await client.sendInput(session: session, text: text)
                    try await client.attentionDismiss(id: h.itemId)
                case .dismiss(let id): try await client.attentionDismiss(id: id)
                case .snooze(let id):
                    let until = UInt64((Date().timeIntervalSince1970 + KeyGrammar.snoozeInterval) * 1000)
                    try await client.attentionSnooze(id: id, untilMs: until)
                }
                receipt = ok ? .sent("\(h.who) · \(h.label)") : .stale
                Log.write("committed \(h.action) ok=\(ok)")
            } catch {
                receipt = .failed("\(error)")
                Log.write("commit failed: \(error)")
            }
            refetch()
            expireReceipt(after: 3)
        }
    }

    private func expireReceipt(after s: TimeInterval) {
        receiptTimer?.invalidate()
        receiptTimer = Timer.scheduledTimer(withTimeInterval: s, repeats: false) { [weak self] _ in
            Task { @MainActor in
                guard let self, self.undo.held == nil else { return }
                self.receipt = nil
            }
        }
    }
}
