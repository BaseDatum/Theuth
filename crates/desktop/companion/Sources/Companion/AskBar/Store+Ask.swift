import AppKit
import CompanionCore
import TheuthKit

struct AskError: Error, CustomStringConvertible {
    let description: String
}

/// W3: the Ask bar's routing (D7, R4: always theuth's desk), the desk's streamed reply, the Done zone and R.
extension Store {
    // MARK: R · ⌘L · the Done zone (keys the W1 grammar did not have)

    /// Intents this file owns; `true` when handled.
    func performAsk(_ intent: Intent) -> Bool {
        switch intent {
        case .reply:
            if let c = focusedCard, c.replyAction("") != nil { ask.replyFor = c.item.id }
            return true
        case .askBar:
            onAskBar?()
            return true
        case .escape where ask.replyFor != nil:
            ask.replyFor = nil
            return true
        case .clear where focusedReceipt != nil:
            if let r = focusedReceipt { ask.receipts.clear(id: r.id); setReceiptCount(ask.receipts.count) }
            return true
        case .openSession where focusedReceipt != nil, .openBoard where focusedReceipt != nil:
            if let s = focusedReceipt?.session { onOpenURL?(DeepLink.session(s)) }
            return true
        default:
            return false
        }
    }

    var focusedReceipt: CompanionCore.Receipt? {
        guard let i = panel.receiptFocus, i < ask.receipts.entries.count else { return nil }
        return ask.receipts.entries[i]
    }

    /// The typed R reply; empty is refused. Goes through the same hold and E5 guard as a keystroke.
    func submitReply(_ text: String) {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty, let c = focusedCard, let action = c.replyAction(t) else { return }
        ask.replyFor = nil
        hold(Held(itemId: c.item.id, who: c.who, label: "replied", action: action))
    }

    // MARK: the Ask bar

    /// ⏎ in the Ask bar: the same hold (⎋ undoes), then `commitAsk`.
    func submitAsk(_ text: String) {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        hold(Held(itemId: "", who: "theuth", label: t, action: .askTheuth(text: t)))
    }

    /// After the hold: resolve theuth's desk once per session, `send_input(desk, text, [], null, null)`, then
    /// collect the desk's reply from the event stream into the receipt row and the Done zone.
    func commitAsk(_ text: String) async {
        do {
            let session = try await resolveDesk()
            _ = try await client.sendInput(session: session, text: text)
            let id = "ask:\(UUID().uuidString)"
            ask.router.sent(on: session)
            ask.current = id
            ask.receipts.add(CompanionCore.Receipt(id: id, kind: .ask, at: Date(), who: "theuth", text: text, session: session), now: Date())
            setReceiptCount(ask.receipts.count)
            setReceipt(.reply(who: "theuth", text: "sent — waiting for the desk", session: session))
            AskLog.write("ask sent to desk \(session) (\(text.count) chars)")
        } catch {
            ask.router.forget() // a refused or closed desk is resolved again on the next send
            setReceipt(.failed("\(error)"))
            expireReceipt(after: 6)
            AskLog.write("ask failed: \(error)")
        }
    }

    /// `mesh_status` → on the hub `desk_open("theuth")`; off it, a replicated desk id or a refusal. Cached.
    func resolveDesk() async throws -> String {
        if let s = ask.router.deskSession { return s }
        if case .unavailable(let why) = ask.router.desk { throw AskError(description: why) }
        defer { focusDeltas() }
        let mesh = try? await client.meshStatus() // engines without a mesh actor are their own hub
        switch AskRouter.plan(mesh: mesh, agents: agents) {
        case .openHere:
            let s = try await client.deskOpen(name: "theuth")
            ask.router.resolved(session: s, hub: true)
            AskLog.write("desk_open(theuth) = \(s) hub=true self=\(mesh?.selfName ?? "no mesh") holder=\(mesh?.lease.holder ?? "nil")")
            return s
        case .use(let s):
            ask.router.resolved(session: s, hub: false)
            AskLog.write("desk from agents_list = \(s) hub=false (hub is \(mesh?.hubName ?? "?"))")
            return s
        case .refuse(let why):
            ask.router.refused(why)
            AskLog.write("desk refused: \(why)")
            throw AskError(description: why)
        }
    }

    // MARK: the reply stream and the Done zone

    private struct Delta: Decodable { let type: String; let session: String?; let text: String? }

    /// 129 §3.2: a connection that never sets a focus gets every session's deltas; ours wants the desk's only.
    /// Re-applied on every `PendingPrompts` (the first item of each (re)subscription); pre-129 engines refuse, fine.
    private func focusDeltas() {
        guard let s = ask.router.deskSession else { return }
        Task {
            do { try await client.setEventFocus([s]) } catch { return Log.write("set_event_focus failed: \(error)") }
            if AskFlags.probe != nil { AskLog.write("set_event_focus([\(s.prefix(8))…]) ok") }
        }
    }

    /// Every event, raw: `TextDelta`/`TurnFinished` on the desk feed the reply while one is being collected.
    func askObserve(_ data: Data) {
        guard let d = try? TheuthJSON.decoder.decode(Delta.self, from: data) else { return }
        if d.type == "PendingPrompts" { return focusDeltas() }
        guard let r = ask.router.reply, !r.finished, let session = d.session else { return }
        let event: ReplyEvent
        switch d.type {
        case "TextDelta": event = .textDelta(session: session, text: d.text ?? "")
        case "TurnFinished": event = .turnFinished(session: session)
        default: return
        }
        guard ask.router.observe(event), let reply = ask.router.reply else { return }
        // ponytail: one publish per delta; coalesce on a timer if the stream ever makes the panel stutter.
        let text = reply.firstTwoLines
        if let id = ask.current { ask.receipts.update(id: id, text: text) }
        setReceipt(.reply(who: "theuth", text: text.isEmpty ? "…" : text, session: reply.session))
        if reply.finished { expireReceipt(after: 12) }
    }

    /// The last 24 h of `user@companion` activity (E4) → the Done zone; runs after every refetch.
    func askRefetchReceipts() {
        Task {
            let since = UInt64((Date().timeIntervalSince1970 - ReceiptModel.window) * 1000)
            do {
                let entries = try await client.activityList(ActivityFilter(sinceMs: since, limit: 200))
                ask.receipts.merge(activity: entries, now: Date())
                setReceiptCount(ask.receipts.count)
                if AskFlags.probe != nil, !activityLogged {
                    activityLogged = true
                    AskLog.write("activity_list(24 h) = \(entries.count) entries, \(entries.filter { $0.actor == CompanionCore.Receipt.companionActor }.count) by \(CompanionCore.Receipt.companionActor) → receipts=\(ask.receipts.count)")
                }
            } catch {
                Log.write("activity_list failed: \(error)")
            }
        }
    }
}

@MainActor private var activityLogged = false

/// stderr plus `/tmp/130-w3-probe.log` during `--ask-probe`.
enum AskLog {
    static let file = URL(fileURLWithPath: "/tmp/130-w3-probe.log")

    static func write(_ s: String) {
        Log.write(s)
        guard AskFlags.probe != nil else { return }
        let line = "[companion \(Date()) pid \(getpid())] \(s)\n"
        if let h = try? FileHandle(forWritingTo: file) {
            h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close()
        } else {
            try? Data(line.utf8).write(to: file)
        }
    }
}

/// `--ask`: open the Ask bar at launch (key). `--ask-probe N`: open it without key, text pre-filled, resolve the
/// desk, log, quit after N s — never sends, never records.
enum AskFlags {
    static let ask = CommandLine.arguments.contains("--ask")
    static let probe: TimeInterval? = {
        let a = CommandLine.arguments
        guard let i = a.firstIndex(of: "--ask-probe"), i + 1 < a.count else { return nil }
        return TimeInterval(a[i + 1])
    }()
    /// Any automated run: no hotkey, no microphone, no sends.
    static var automated: Bool { Flags.probe != nil || probe != nil }
}
