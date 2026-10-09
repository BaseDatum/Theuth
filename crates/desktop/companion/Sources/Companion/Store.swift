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
        /// W3: the desk's streamed answer in the receipt row ("theuth · handed to w3 — …" · Open ⌘O).
        case reply(who: String, text: String, session: String?)
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
    /// W3: Ask bar routing, the Done zone's receipts, the R field (Store+Ask.swift).
    @Published var ask = AskModel()
    // 149 C: joined the 250 ms fetch set; W1's agents panel reads them (nil until the engine answers).
    @Published private(set) var sessions: [SessionSummary] = []
    @Published private(set) var sessionsStatus: SessionsStatus?
    @Published private(set) var meshStatus: MeshStatus?
    /// 149 W3: the island's slice (`MeetingReducer`); nil until W3 seeds it from `meeting_status`.
    @Published var meeting: MeetingSlice?
    /// 149 W2: point-and-say's live state; nil while idle.
    @Published var point: PointSlice?
    /// 149s S3: one "currently speaking" for the voice key and the Ask bar (`Store+Ask.swift` speaks the Ask reply,
    /// `PointController` the voice key's); the player is the engine's `Speech*` sink.
    let speech = SpeechPlayer()
    lazy var speaker: Speaker = {
        let s = Speaker(sink: speech)
        s.log = { Log.write("speech: \($0)") }
        speech.onEnded = { [s] id in s.ended(id) }
        return s
    }()

    private(set) var undo = UndoQueue<Held>(delay: Settings.undoDelay)
    private(set) var client: RpcClient!
    private var refetchTimer: Timer?
    private var tickTimer: Timer?
    private var receiptTimer: Timer?
    private var fetchGate = FetchGate()

    /// Hooks the panel controller sets.
    var onCloseRequest: (() -> Void)?
    var onOpenURL: ((URL) -> Void)?
    /// Probe hook: the first `PendingPrompts`.
    var onPendingPrompts: ((PendingPrompts) -> Void)?
    /// Probe hook: a full refetch landed.
    var onRefetched: (() -> Void)?
    /// W3: ⌘L from the Attention panel.
    var onAskBar: (() -> Void)?
    /// W6: ⌘/ and ⌘, from the panel or the Ask bar.
    var onLegend: (() -> Void)?
    var onSettings: (() -> Void)?
    /// W6: a `Dictation*` event was reduced (`takeText`: the field should show `liveText`); the raw line, for the probe.
    var onDictation: ((AskDictation, Bool) -> Void)?
    var onDictationWire: ((String) -> Void)?

    /// `locate` runs before every dial, so the engine is found whenever (and wherever) it comes up.
    func start(locate: @escaping @Sendable () -> EngineLocation?) {
        client = RpcClient(locate: locate,
                           onState: { s in Task { @MainActor in self.connection = s } },
                           onEvent: { d in Task { @MainActor in self.handle(d) } })
        Task {
            await client.start()
            await client.subscribeEvents()
        }
        speaker.client = client
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
        askObserve(data)
        meetingObserve(data) // 149 W3: MeetingEvent.decode(data) → MeetingReducer.reduce(meeting, …)
        speech.observe(data) // 149s: SpeechStarted / SpeechAudio / SpeechFinished for the shared speaker's utterance
        pointObserve(data) // 149 W2: the voice key's dictation and the desk's reply
    }

    /// Debounced 250 ms (130 §5.2); never on a timer.
    private func scheduleRefetch(now immediate: Bool = false) {
        refetchTimer?.invalidate()
        if immediate { return refetch() }
        refetchTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.refetch() }
        }
    }

    /// The three lists at once (+ 149's sessions/status/mesh, each tolerated missing on an older engine), then the packets;
    /// one in flight, a request during it runs once after it.
    func refetch() {
        guard !w1Frozen, fetchGate.begin() else { return } // 149 W1: a seeded probe never takes the engine's lists
        Task {
            do {
                async let a = client.attentionList()
                async let g = client.agentsList()
                async let l = client.listActivity()
                // 149: optional on an engine without these verbs — a failure leaves the last value.
                async let s = try? client.listSessions()
                async let ss = try? client.sessionsStatus()
                async let m = try? client.meshStatus()
                let (items, agents, activity) = try await (a, g, l)
                apply(items: items, agents: agents, activity: activity)
                if let s = await s { sessions = s }
                if let ss = await ss { sessionsStatus = ss }
                if let m = await m { meshStatus = m }
                await loadPackets()
                onRefetched?()
            } catch {
                Log.write("refetch failed: \(error)")
            }
            if fetchGate.end() { refetch() }
        }
    }

    private func apply(items: [AttentionItem], agents: [AgentSummary], activity: [AgentInfo]) {
        self.items = items
        self.agents = agents
        self.activity = activity
        rail = RailModel.derive(items: items, agents: agents, activity: activity, now: Date())
        panel.workingCount = rail.workingCount
        panel.update(items: items)
        askRefetchReceipts()
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
        if performAsk(intent) { return }
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
        case .legend: onLegend?()
        case .settings: onSettings?()
        case .none, .send: break
        case .reply, .askBar: break // W3: handled in performAsk
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

    /// Hold `h` for the undo delay, then commit it. A hold while another is held commits the displaced one at
    /// once (130 §5.4); the delay setting is read at every hold.
    func hold(_ h: Held) {
        if let displaced = undo.hold(h, now: Date(), delay: Settings.undoDelay) { commit(displaced) }
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
            if case .askTheuth(let text) = h.action { return await commitAsk(text) }
            switch await Commit.run(itemId: h.itemId, action: h.action, engine: client, now: Date()) {
            case .skipped: return
            case .gone(let latest):
                receipt = .stale
                apply(items: latest, agents: agents, activity: activity)
                return expireReceipt(after: 3)
            case .sent:
                receipt = .sent("\(h.who) · \(h.label)")
                Log.write("committed \(h.action) ok=true")
            case .stale:
                receipt = .stale
                Log.write("committed \(h.action) ok=false")
            case .failed(let error):
                receipt = .failed(error)
                Log.write("commit failed: \(error)")
            }
            refetch()
            expireReceipt(after: 3)
        }
    }

    func expireReceipt(after s: TimeInterval) {
        receiptTimer?.invalidate()
        receiptTimer = Timer.scheduledTimer(withTimeInterval: s, repeats: false) { [weak self] _ in
            Task { @MainActor in
                guard let self, self.undo.held == nil else { return }
                self.receipt = nil
            }
        }
    }

    /// W3: the Ask bar's receipts share this row.
    func setReceipt(_ r: Receipt?) { receipt = r }
    func setReceiptCount(_ n: Int) { panel.receiptCount = n }
    // MARK: W2 notifications

    /// A deliberate tap on a notification action (D8): the same commit path, no hold; an optional note
    /// goes to the session as `send_input` (R1-5's "deny with a note").
    func answerNow(_ h: Held, note: String? = nil, session: String? = nil) {
        commit(h)
        guard let note, let session else { return }
        Task { _ = try? await client.sendInput(session: session, text: note) }
    }

    // MARK: 149 W1 probes

    /// `--probe-rail/--probe-agents/--probe-panel`: the engine's lists replaced by fixtures; from then on `refetch` is a no-op
    /// (a reachable engine cannot overwrite them) and `connection` reads open so the surfaces render the fixture.
    private var w1Frozen = false
    func w1Seed(items: [AttentionItem], agents: [AgentSummary], activity: [AgentInfo], sessions: [SessionSummary],
                mesh: MeshStatus?, status: SessionsStatus?, packets: [String: Packet] = [:]) {
        w1Frozen = true
        self.items = items; self.agents = agents; self.activity = activity; self.sessions = sessions
        meshStatus = mesh; sessionsStatus = status
        for (k, v) in packets { self.packets[k] = v }
        rail = RailModel.derive(items: items, agents: agents, activity: activity, now: Date())
        panel.workingCount = rail.workingCount
        panel.update(items: items)
        connection = .open
    }
}
