import AppKit
import Combine
import CompanionCore
import KeyboardShortcuts
import SwiftUI
import TheuthKit

// 149 W1 — §3.2 the agents panel grouped by host; §5.4 A per-row talk; §8 agents-scope keys; §10 `--probe-agents`.

/// What a row's stop or mic holds for the undo delay (§3.2 "Per-row ✕", "Per-row mic").
enum RowAction: Equatable {
    case stop(session: String, name: String, children: Int)
    case pauseDesk(name: String)
    case send(session: String, name: String, text: String)

    /// The hold pill's tint: red for stop/pause, green for a send.
    var tint: Color { if case .send = self { return .done } else { return .failed } }
}

/// The panel's own state; `groups` is re-derived from the Store, the rest is this surface's (focus, hold, reply field).
@MainActor
final class AgentsPanelState: ObservableObject {
    enum RowEvent { case focus(AgentRow), open(AgentRow), micDown(AgentRow), micUp, stop(AgentRow), submitReply, close }

    @Published var groups: [HostGroup] = []
    /// Flat index over every group's rows.
    @Published var focus = 0
    /// The row id whose reply field is open.
    @Published var replyFor: String?
    @Published var replyText = ""
    @Published var hold = UndoQueue<RowAction>()
    /// The hold clock (ticked every 0.1 s while something is held).
    @Published var now = Date()
    @Published var pill: String?
    @Published var receipt: String?
    @Published var receiptColor: Color = .done
    @Published var scrollTarget: String?
    /// The controller's hook for row buttons.
    var onEvent: ((RowEvent) -> Void)?

    var rows: [AgentRow] { groups.flatMap(\.rows) }
    var focusedRow: AgentRow? { rows.indices.contains(focus) ? rows[focus] : nil }
    func row(session: String) -> AgentRow? { rows.first { $0.session == session } }
}

private extension AgentRow {
    /// `agents_list` desks carry `agent:<name>` ids (stop = `agent_pause`, not `cancel`).
    var isDesk: Bool { id.hasPrefix("agent:") }
    var deskName: String { String(id.dropFirst(6)) }
    var canTalk: Bool { live && !session.isEmpty }
    /// §3.2: a row whose session is not running shows stop disabled; a desk pauses whenever it has a session.
    var canStop: Bool {
        guard live else { return false }
        if isDesk { return !session.isEmpty }
        switch state {
        case .running, .awaiting, .approval: return true
        case .idle, .error, .cancelled: return false
        }
    }
}

/// The agents panel (§3.2): the Attention panel's glass at radius 24, key only when summoned, never activates.
@MainActor
final class AgentsPanelController: NSObject {
    static var shared: AgentsPanelController?
    static let width: CGFloat = 372
    static let maxHeight: CGFloat = 560

    let panel: KeyPanel
    private(set) var isOpen = false
    let state = AgentsPanelState()
    private let store: Store
    private let hover: HoverSync
    private let voice: HoldToTalk
    private let hosting: NSHostingView<AgentsPanelView>
    private var subs = Set<AnyCancellable>()
    private var keyMonitor: Any?
    private var previousApp: NSRunningApplication?
    private var anchor: PanelController.Anchor = .mouse
    private var tickTimer: Timer?
    private var receiptTimer: Timer?

    init(store: Store, hover: HoverSync = W1Hover.shared) {
        self.store = store
        self.hover = hover
        voice = HoldToTalk(store: store)
        let (panel, container) = GlassPanel.make(width: Self.width, height: 240, radius: Type.cardRadius)
        self.panel = panel
        hosting = NSHostingView(rootView: AgentsPanelView(store: store, state: state, hover: hover, voice: voice))
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        super.init()

        state.onEvent = { [weak self] e in self?.handle(e) }
        // §5.4 A: the dictated text lands in the row's reply field and is held for the undo delay before `send_input`.
        voice.onText = { [weak self] text in
            guard let self, let target = self.voice.target, let row = self.state.row(session: target) else { return }
            self.state.replyFor = row.id
            self.state.replyText = text
            self.hold(.send(session: row.session, name: row.name, text: text))
        }
        NotificationCenter.default.publisher(for: NSWindow.didResignKeyNotification, object: panel)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.close(returnKey: false) } }
            .store(in: &subs)
        // `objectWillChange` fires before the change: regroup one run-loop turn later, as `PanelController.relayout`.
        store.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.storeChanged() } }
            .store(in: &subs)
        state.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.relayout() } }
            .store(in: &subs)
        voice.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.relayout() } }
            .store(in: &subs)
    }

    // MARK: install

    /// ⌃⇧A (§8) toggles the panel at the mouse's screen. Probes never open it.
    func installHotkeys() {
        KeyboardShortcuts.onKeyUp(for: .openAgents) { [weak self] in
            guard let self, !AskFlags.automated else { return }
            self.toggle(anchor: .mouse)
        }
    }

    /// "Agents…" after "Ask theuth…" in the status item's menu.
    func addMenuItem(to menu: NSMenu) {
        let item = NSMenuItem(title: "Agents…", action: #selector(agentsFromMenu), keyEquivalent: "a")
        item.keyEquivalentModifierMask = [.control, .shift]
        item.target = self
        menu.insertItem(item, at: min(2, menu.items.count))
    }

    @objc private func agentsFromMenu() { open(anchor: .mouse, scrollTo: nil) }

    // MARK: open / close

    func toggle(anchor: PanelController.Anchor) {
        if isOpen { close(returnKey: true) } else { open(anchor: anchor, scrollTo: nil) }
    }

    /// Summon: remember the frontmost app, place, take key; scrolled to `session` or the first working row (§8).
    func open(anchor: PanelController.Anchor, scrollTo session: String?) {
        self.anchor = anchor
        previousApp = NSWorkspace.shared.frontmostApplication
        isOpen = true
        regroup()
        scroll(to: session ?? AgentsModel.firstWorking(state.groups)?.session)
        relayout(now: true)
        panel.ignoresMouseEvents = false
        panel.makeKeyAndOrderFront(nil)
        if keyMonitor == nil {
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp]) { [weak self] e in
                MainActor.assumeIsolated {
                    guard let self, self.panel.isKeyWindow else { return e }
                    return self.handle(e) ? nil : e
                }
            }
        }
    }

    /// Probe mode: on screen, not key, mouse-transparent, so nothing can lose focus.
    func showWithoutKey(anchor: PanelController.Anchor) {
        self.anchor = anchor
        isOpen = true
        regroup()
        relayout(now: true)
        panel.ignoresMouseEvents = true
        panel.orderFrontRegardless()
    }

    func close(returnKey: Bool) {
        guard isOpen else { return }
        isOpen = false
        if let m = keyMonitor { NSEvent.removeMonitor(m); keyMonitor = nil }
        voice.cancel()
        state.replyFor = nil
        state.replyText = ""
        hover.session = nil
        panel.orderOut(nil)
        if returnKey, let app = previousApp, app != .current { app.activate() }
        previousApp = nil
    }

    private func scroll(to session: String?) {
        guard let session, let i = state.rows.firstIndex(where: { $0.session == session }) else { return }
        state.focus = i
        state.scrollTarget = state.rows[i].id
    }

    // MARK: groups

    private func storeChanged() {
        DispatchQueue.main.async { [self] in
            regroup()
            if isOpen { place() }
        }
    }

    /// `list_activity` ∪ `agents_list` ∪ `list_sessions` + `mesh_status`; focus stays on the same row id when it survives.
    /// While the engine is unreachable the last list stays (greyed by the view, §3.2 "Empty/error").
    private func regroup() {
        if store.connection != .open, !state.groups.isEmpty { return }
        let groups = AgentsModel.groups(activity: store.activity, agents: store.agents, sessions: store.sessions,
                                        mesh: store.meshStatus, status: store.sessionsStatus)
        guard groups != state.groups else { return }
        let keep = state.focusedRow?.id
        state.groups = groups
        let rows = state.rows
        if let keep, let i = rows.firstIndex(where: { $0.id == keep }) { state.focus = i }
        else { state.focus = min(state.focus, max(0, rows.count - 1)) }
        if let r = state.replyFor, !rows.contains(where: { $0.id == r }) { state.replyFor = nil; state.replyText = "" }
    }

    // MARK: rows

    private func handle(_ e: AgentsPanelState.RowEvent) {
        switch e {
        case .focus(let row): if let i = state.rows.firstIndex(where: { $0.id == row.id }) { state.focus = i }
        case .open(let row): openSession(row)
        case .micDown(let row): if row.canTalk { voice.press(target: row.session) }
        case .micUp: voice.release()
        case .stop(let row): stop(row)
        case .submitReply: submitReply()
        case .close: close(returnKey: true)
        }
    }

    private func move(_ delta: Int) {
        let n = state.rows.count
        guard n > 0 else { return }
        state.focus = min(n - 1, max(0, state.focus + delta)) // never wraps
        state.scrollTarget = state.rows[state.focus].id
    }

    /// ⏎ / ⌘O: `theuth://c/<session>`; the Store's hook opens it (and closes the Attention panel), then this closes.
    private func openSession(_ row: AgentRow) {
        guard !row.session.isEmpty else { return }
        store.onOpenURL?(DeepLink.session(row.session))
        close(returnKey: false)
    }

    /// Stop / X: the hold IS the confirmation (§3.2, Revision 1 row 2); desks pause, sessions cancel the tree.
    private func stop(_ row: AgentRow) {
        guard row.canStop else { return }
        if row.isDesk { hold(.pauseDesk(name: row.deskName)) }
        else { hold(.stop(session: row.session, name: row.name, children: row.children)) }
    }

    /// ⏎ in the reply field: (re-)hold the send.
    private func submitReply() {
        let text = state.replyText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let id = state.replyFor, let row = state.rows.first(where: { $0.id == id }), !text.isEmpty, row.canTalk else { return }
        hold(.send(session: row.session, name: row.name, text: text))
    }

    // MARK: hold / commit (mirrors `Store.hold/tick/commit`)

    static func pillText(_ a: RowAction) -> String {
        switch a {
        case .stop(_, let name, let n):
            if n == 0 { return "Stopping \(name) · ⎋ undo" }
            return "Stopping \(name) and \(n) \(n == 1 ? "child" : "children") · ⎋ undo"
        case .pauseDesk(let name): return "Pausing \(name) · ⎋ undo"
        case .send(_, let name, _): return "sending to \(name) · ⎋ undo"
        }
    }

    private func hold(_ a: RowAction) {
        if let displaced = state.hold.hold(a, now: Date(), delay: Settings.undoDelay) { commit(displaced) }
        state.now = Date()
        state.pill = Self.pillText(a)
        state.receipt = nil
        receiptTimer?.invalidate()
        tickTimer?.invalidate()
        tickTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.tick() }
        }
    }

    private func tick() {
        state.now = Date()
        if let fired = state.hold.tick(now: state.now) {
            stopTicking()
            state.pill = nil
            commit(fired)
        } else if state.hold.held == nil {
            stopTicking()
        }
    }

    private func stopTicking() { tickTimer?.invalidate(); tickTimer = nil }

    /// A cancel is final: the receipt after commit never says "undo" (D3, recon C C2.7). Probes never act.
    private func commit(_ a: RowAction) {
        guard !AskFlags.automated else { return }
        if case .send = a { state.replyFor = nil; state.replyText = "" }
        Task {
            do {
                switch a {
                case .stop(let session, let name, _):
                    try await store.client.cancel(session: session, scope: .tree)
                    receipt("stopped · \(name)")
                case .pauseDesk(let name):
                    try await store.client.agentPause(name: name, paused: true)
                    receipt("paused · \(name)")
                case .send(let session, let name, let text):
                    _ = try await store.client.sendInput(session: session, text: text, files: [], delivery: nil, id: nil)
                    receipt("sent · \(name)")
                }
                Log.write("agents committed \(a) ok=true")
            } catch {
                receipt("Failed · \(error)", color: .failed)
                Log.write("agents commit failed: \(error)")
            }
        }
    }

    private func receipt(_ text: String, color: Color = .done, after s: TimeInterval = 3) {
        state.receipt = text
        state.receiptColor = color
        receiptTimer?.invalidate()
        receiptTimer = Timer.scheduledTimer(withTimeInterval: s, repeats: false) { [weak self] _ in
            Task { @MainActor in
                guard let self, self.state.hold.held == nil else { return }
                self.state.receipt = nil
            }
        }
    }

    /// ⎋: take back a hold (the reply field stays — "⎋ keeps editing"), else drop the field, else close.
    private func escape() {
        if state.hold.cancel() {
            stopTicking()
            state.pill = nil
            receipt("Taken back", color: .secondary, after: 1.5)
            return
        }
        if voice.isBusy { voice.cancel(); return }
        if state.replyFor != nil { state.replyFor = nil; state.replyText = ""; return }
        close(returnKey: true)
    }

    // MARK: keys (§8, scope `.agents`)

    /// Key-down and key-up (V is a hold). While the reply field has focus only ⎋ and ⏎ are ours.
    private func handle(_ e: NSEvent) -> Bool {
        guard let action = SettingsModel.shared.keys.action(for: LegendController.press(e).chord, in: .agents) else { return false }
        let typing = state.replyFor != nil
        if e.type == .keyUp {
            guard action == .agentsTalk, !typing else { return false }
            voice.release()
            return true
        }
        switch action {
        case .agentsEscape: escape()
        case .agentsOpenSession where typing: submitReply()
        case _ where typing: return false
        case .agentsNext: move(1)
        case .agentsPrevious: move(-1)
        case .agentsOpenSession, .agentsOpenSessionCmd: if let row = state.focusedRow { openSession(row) }
        case .agentsTalk:
            guard !e.isARepeat, let row = state.focusedRow, row.canTalk else { return true }
            voice.press(target: row.session)
        case .agentsStop: if let row = state.focusedRow { stop(row) }
        default: return false
        }
        return true
    }

    // MARK: placement (`PanelController.origin(for:)`'s logic; it is private there)

    private func relayout(now: Bool = false) {
        guard isOpen else { return }
        if now { return place() }
        DispatchQueue.main.async { [self] in if isOpen { place() } }
    }

    private func place() {
        hosting.layoutSubtreeIfNeeded()
        let fit = hosting.fittingSize
        let size = NSSize(width: Self.width, height: min(Self.maxHeight, max(120, fit.height)))
        panel.setFrame(NSRect(origin: origin(for: size), size: size), display: true)
        hosting.frame = panel.contentView?.bounds ?? .zero
    }

    private func origin(for size: NSSize) -> NSPoint {
        let screen: NSScreen
        var p: NSPoint
        switch anchor {
        case .statusItem(let f):
            screen = NSScreen.screens.first { $0.frame.contains(NSPoint(x: f.midX, y: f.midY)) } ?? NSScreen.main!
            p = NSPoint(x: f.midX - size.width / 2, y: f.minY - size.height - 4)
        case .rail(let f):
            screen = NSScreen.screens.first { $0.frame.contains(NSPoint(x: f.midX, y: f.midY)) } ?? NSScreen.main!
            let x = Settings.railSide == "left" ? f.maxX + 8 : f.minX - 8 - size.width
            p = NSPoint(x: x, y: f.midY - size.height / 2)
        case .mouse:
            let m = NSEvent.mouseLocation
            screen = NSScreen.screens.first { $0.frame.contains(m) } ?? NSScreen.main!
            let v = screen.visibleFrame
            p = NSPoint(x: v.midX - size.width / 2, y: v.maxY - v.height * 0.2 - size.height)
        }
        let v = screen.visibleFrame
        p.x = min(max(v.minX + 8, p.x), v.maxX - size.width - 8)
        p.y = min(max(v.minY + 8, p.y), v.maxY - size.height - 8)
        return p
    }
}

// MARK: - views (mock round 2, screen 2)

/// Header · host groups · the hold pill · the receipt line · footer hints.
struct AgentsPanelView: View {
    @ObservedObject var store: Store
    @ObservedObject var state: AgentsPanelState
    @ObservedObject var hover: HoverSync
    @ObservedObject var voice: HoldToTalk

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            if store.connection != .open {
                Text("Connecting to the engine…").font(Type.meta).foregroundColor(.tertiary).padding(.horizontal, 14).padding(.bottom, 4)
            }
            if state.groups.isEmpty { empty } else { list.opacity(store.connection == .open ? 1 : 0.5) }
            if let pill = state.pill, let held = state.hold.held {
                HoldPill(text: pill, tint: held.action.tint, remaining: state.hold.remaining(now: state.now), delay: state.hold.delay)
            }
            if let r = state.receipt {
                Text(r).font(Type.meta).foregroundColor(state.receiptColor).lineLimit(1).padding(.horizontal, 14).padding(.top, 6)
            }
            footer
        }
        .frame(width: AgentsPanelController.width)
        .foregroundColor(.primary)
    }

    private var header: some View {
        HStack(spacing: 10) {
            Text("Agents").font(Type.body).fontWeight(.semibold)
            Spacer()
            Hint("J K", "move")
            Button { state.onEvent?(.close) } label: {
                HStack(spacing: 4) {
                    Image(systemName: "xmark").font(.caption)
                    Text("esc").font(Type.mono).padding(.horizontal, 4).padding(.vertical, 1)
                        .background(RoundedRectangle(cornerRadius: Type.capRadius).stroke(Color.separator, lineWidth: 1))
                }.foregroundColor(.secondary)
            }.buttonStyle(.borderless).help("Close").accessibilityLabel("Close")
        }
        .padding(.horizontal, 14).padding(.top, 12).padding(.bottom, 6)
    }

    private var list: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(state.groups) { g in
                        Text(g.title).font(Type.caps).foregroundColor(.tertiary).padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 2)
                        ForEach(g.rows) { row in
                            let recording = voice.target == row.session && (voice.recording || voice.finishing)
                            AgentRowView(row: row, peer: g.node ?? g.title, focused: state.focusedRow?.id == row.id, recording: recording,
                                         levels: voice.levels, hover: hover) { state.onEvent?($0) }
                                .id(row.id)
                            if state.replyFor == row.id || recording {
                                AgentReplyField(name: row.name, text: $state.replyText,
                                                live: recording ? voice.liveText + voice.partial : nil) { state.onEvent?(.submitReply) }
                            }
                        }
                    }
                }
            }
            .onChange(of: state.scrollTarget) { _, id in if let id { proxy.scrollTo(id, anchor: .center) } }
            .onAppear { if let id = state.scrollTarget { proxy.scrollTo(id, anchor: .center) } }
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("No agents running.").font(Type.body).foregroundColor(.secondary)
            Text("Desks appear here when they wake; Ask theuth starts one.").font(Type.meta).foregroundColor(.tertiary)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Hint("J K", "move"); Hint("⏎", "open"); Hint("V", "talk"); Hint("X", "stop"); Hint("⎋", "close")
        }
        .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 10)
    }
}

/// dot/spinner · harness icon · name · children · last · mic · stop (§3.2). Focused: accent wash + 3 pt bar.
struct AgentRowView: View {
    let row: AgentRow
    /// The group's node name — a remote row's "on ‹peer›" suffix and tooltip.
    let peer: String
    let focused: Bool
    let recording: Bool
    let levels: [Float]
    @ObservedObject var hover: HoverSync
    let onEvent: (AgentsPanelState.RowEvent) -> Void
    @State private var xHover = false
    @State private var pressing = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 8) {
            dot.frame(width: 10, height: 10)
            HarnessIcon(key: row.harness)
            Text(row.name).font(Type.body).lineLimit(1).layoutPriority(1)
            if row.children > 0 {
                HStack(spacing: 2) { Image(systemName: "arrow.triangle.branch"); Text("\(row.children)") }
                    .font(Type.mono).foregroundColor(.secondary)
            }
            if !row.live { Text("on \(peer)").font(Type.caps).foregroundColor(.tertiary).lineLimit(1) }
            if !row.last.isEmpty { Text(row.last).font(Type.meta).foregroundColor(.secondary).lineLimit(1) }
            Spacer(minLength: 4)
            mic
            stop
        }
        .padding(.leading, 14).padding(.trailing, 8)
        .frame(height: 36)
        .background(focused ? Color.accentColor.opacity(0.12)
                    : hover.session == row.session && !row.session.isEmpty ? Color.primary.opacity(0.05) : Color.clear)
        .overlay(alignment: .leading) { if focused { Rectangle().fill(Color.accentColor).frame(width: 3) } }
        .contentShape(Rectangle())
        .onHover { inside in
            if inside { hover.session = row.session } else if hover.session == row.session { hover.session = nil }
        }
        .onTapGesture { onEvent(.focus(row)) }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(row.name), \(row.state.word)\(row.live ? "" : ", on \(peer)")")
    }

    /// A remote row is a hollow tertiary dot whatever its state: this engine has no live state for it (§3.2 "honestly").
    @ViewBuilder private var dot: some View {
        if !row.live {
            Circle().strokeBorder(Color.tertiary, lineWidth: 1.5)
        } else {
            switch row.state {
            case .running: if reduceMotion { MarkView(state: .working) } else { ProgressView().controlSize(.mini) }
            case .awaiting, .approval: MarkView(state: .needsYou(.ask))
            case .idle, .cancelled: MarkView(state: .idle)
            case .error: MarkView(state: .error)
            }
        }
    }

    private var disabledHelp: String { row.live ? "nothing is running" : "open it on \(peer)" }

    private var mic: some View {
        Group {
            if recording { LevelBars(levels: levels, height: 12) }
            else { Image(systemName: "mic").font(.callout).foregroundColor(.secondary) }
        }
        .frame(width: 28, height: 24)
        .contentShape(Rectangle())
        .opacity(row.canTalk ? 1 : 0.35)
        .help(row.canTalk ? "Hold to talk to \(row.name)" : disabledHelp)
        .allowsHitTesting(row.canTalk)
        .gesture(DragGesture(minimumDistance: 0)
            .onChanged { _ in if !pressing { pressing = true; onEvent(.micDown(row)) } }
            .onEnded { _ in pressing = false; onEvent(.micUp) })
        .accessibilityLabel(recording ? "Listening" : "Hold to talk")
    }

    private var stop: some View {
        Button { onEvent(.stop(row)) } label: {
            Image(systemName: "xmark").font(.subheadline.weight(.semibold))
                .foregroundColor(xHover && row.canStop ? .failed : .tertiary)
                .frame(width: 24, height: 24).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!row.canStop)
        .opacity(row.canStop ? 1 : 0.35)
        .onHover { xHover = $0 }
        .help(row.canStop ? (row.isDesk ? "Pause \(row.name)" : "Stop \(row.name)") : disabledHelp)
    }
}

/// Under a row: the dictation's live text while recording, else the reply field for the 3 s hold ("⎋ keeps editing").
struct AgentReplyField: View {
    let name: String
    @Binding var text: String
    /// Non-nil while recording: `liveText + partial`, muted.
    let live: String?
    let onSubmit: () -> Void
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let live {
                Text(live.isEmpty ? "listening…" : live).font(Type.meta).foregroundColor(.secondary).lineLimit(2)
            } else {
                TextField("Reply to \(name)", text: $text)
                    .textFieldStyle(.roundedBorder).font(Type.meta)
                    .focused($focused)
                    .onSubmit(onSubmit)
                    .onAppear { if !AskFlags.automated { focused = true } } // a probe's panel is never key
            }
            Text("⏎ send · ⎋ keeps editing").font(Type.caps).foregroundColor(.tertiary)
        }
        .padding(.leading, 40).padding(.trailing, 14).padding(.vertical, 6)
    }
}

/// "Stopping w3-149-island and 2 children · ⎋ undo" with the 2 pt countdown (as `ReceiptView`'s bar).
struct HoldPill: View {
    let text: String
    let tint: Color
    let remaining: TimeInterval
    let delay: TimeInterval
    private let width = AgentsPanelController.width - 28

    var body: some View {
        Text(text).font(Type.meta).lineLimit(1)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .frame(width: width, alignment: .leading)
            .background(Capsule().fill(tint.opacity(0.15)))
            .overlay(alignment: .bottomLeading) {
                if delay > 0 {
                    Capsule().fill(tint).frame(width: max(0, (width - 24) * CGFloat(min(1, remaining / delay))), height: 2).padding(.horizontal, 12)
                }
            }
            .padding(.horizontal, 14).padding(.top, 8)
    }
}

// MARK: - probe (§10)

/// `--probe-agents N`: two hosts seeded in place of the engine, one row in its stop hold, one with its reply field open;
/// shown without key for N s, logged, quit. Nothing sent, no microphone (`AskFlags.automated`).
enum AgentsProbe {
    @MainActor private static var subs = Set<AnyCancellable>()

    @MainActor
    static func run(seconds: TimeInterval, store: Store, agents: AgentsPanelController) {
        let now = Date()
        let mesh = MeshStatus(selfId: "engine-mbp", selfName: "mbp",
                              peers: [.init(id: "engine-studio", name: "studio", reachable: true)], lease: .init(holder: nil))
        let sessions = [
            SessionSummary(id: "s-1", title: "w1-149-parity", agent: "omp", updatedAt: now, running: true),
            SessionSummary(id: "s-3", title: "w3-149-island", agent: "omp", updatedAt: now, running: true),
            SessionSummary(id: "s-4", title: "docs-149 writer", agent: "opencode", updatedAt: now - 900, running: false),
            SessionSummary(id: "s-d", title: "theuth · desk", agent: "theuth", desk: "theuth", updatedAt: now, running: true),
            SessionSummary(id: "s-9", title: "134-W7 WinUI rail", agent: "opencode", updatedAt: now, running: false, owner: "engine-studio"),
        ]
        let activity = [
            AgentInfo(id: "s-1", name: "w1-149-parity", agent: "omp", state: .running, startedAt: now - 240,
                      last: "building RailPills.swift",
                      activity: .init(state: .running, lastActive: now, turnStartedAt: now - 240, live: true)),
            AgentInfo(id: "s-3", name: "w3-149-island", agent: "omp", state: .running, startedAt: now - 600,
                      last: "wiring the island's request box",
                      activity: .init(state: .running, lastActive: now, turnStartedAt: now - 600, children: 2, live: true)),
            AgentInfo(id: "s-4", name: "docs-149 writer", agent: "opencode", state: .idle, startedAt: now - 3600,
                      finishedAt: now - 900, last: "finished §3.2 draft",
                      activity: .init(state: .idle, lastActive: now - 900)),
        ]
        let desks = [AgentSummary(file: AgentFile(name: "theuth", title: "theuth"), status: .working, desk: "s-d")]
        let seed: () -> Void = { store.w1Seed(items: [], agents: desks, activity: activity, sessions: sessions, mesh: mesh, status: nil) }
        seed()
        store.onRefetched = seed
        // A reachable engine must never overwrite the fixture; `w1Seed` sets `.open` itself, so only other states re-seed.
        store.$connection.removeDuplicates().receive(on: RunLoop.main)
            .sink { s in MainActor.assumeIsolated { if s != .open { seed() } } }
            .store(in: &subs)

        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
            agents.showWithoutKey(anchor: .mouse)
            let state = agents.state
            if let row = state.row(session: "s-3") {
                let action = RowAction.stop(session: row.session, name: row.name, children: row.children)
                // No tick timer runs, so the hold never fires (and `commit` is a no-op under `AskFlags.automated` anyway).
                state.hold.hold(action, now: Date(), delay: seconds)
                state.now = Date()
                state.pill = AgentsPanelController.pillText(action)
            }
            if let row = state.row(session: "s-1") {
                state.replyFor = row.id
                state.replyText = "ship the pills first"
            }
            let remote = state.rows.filter { !$0.live }.map(\.name)
            W1Log.write("probe-agents frame=\(agents.panel.frame) visible=\(agents.panel.isVisible) key=\(agents.panel.isKeyWindow) groups=\(state.groups.map { "\($0.title):\($0.rows.count)" }) remote=\(remote) pill=\"\(state.pill ?? "")\" replyFor=\(state.replyFor ?? "nil")")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
            // Ground truth from the window server (top-left origin), as `AppDelegate.runProbe` reads it.
            let wins = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
                .filter { ($0[kCGWindowOwnerPID as String] as? Int32) == getpid() }
                .compactMap { $0[kCGWindowBounds as String] as? [String: Any] }
                .map { "x=\($0["X"] ?? 0) y=\($0["Y"] ?? 0) w=\($0["Width"] ?? 0) h=\($0["Height"] ?? 0)" }
            W1Log.write("probe-agents windows (CGWindowList): \(wins)")
            W1Log.write("probe-agents done after \(seconds) s — quitting (frame=\(agents.panel.frame) active=\(NSApp.isActive) key=\(agents.panel.isKeyWindow))")
            NSApp.terminate(nil)
        }
    }
}
