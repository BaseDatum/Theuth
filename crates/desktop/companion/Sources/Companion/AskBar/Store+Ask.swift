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

    /// ⏎ in the Ask bar: the same hold (⎋ undoes), then `commitAsk`. S3: a new ask silences the reply still being spoken.
    func submitAsk(_ text: String) {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        speaker.stop()
        hold(Held(itemId: "", who: "theuth", label: t, action: .askTheuth(text: t)))
    }

    /// 153 §7.4 "send it": the phrase was the confirmation — no 3 s hold, straight to `commitAsk`.
    func sendAskNow(_ text: String) {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        speaker.stop()
        Task { await commitAsk(t) }
    }

    /// S3: the Ask bar's toggle, or `--probe-speak`'s override (a probe never writes the owner's defaults).
    var askSpeakEnabled: Bool { AskFlags.probeSpeak || SpokenReplies.ask(Settings.defaults) }

    /// After the hold: resolve theuth's desk once per session, `send_input(desk, text, [], null, null)`, then
    /// collect the desk's reply from the event stream into the receipt row and the Done zone. 153 §6.1: when the text came
    /// from a dictation (`correctionId`), `dictation_correction(id, sent_text)` goes out beside it — the engine diffs what was
    /// dictated against what was sent; §3.3: the reply also drives the answer card (`showAnswerCard`, C1's view).
    func commitAsk(_ text: String) async {
        guard !AskFlags.automated else { return AskLog.write("probe: send suppressed (commit)") }
        if let dictated = ask.dictation.correctionId {
            _ = ask.dictation.handle(.settled)
            Task { [client = client!] in
                do { try await client.dictationCorrection(id: dictated, sentText: text); AskLog.write("dictation_correction(\(dictated)) sent") }
                catch { Log.write("dictation_correction failed: \(error)") }
            }
        }
        askCard.begin(question: text)
        askCard.phase = .thinking
        askCard.status = "thinking"
        showAnswerCard(askCard)
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
            askCard.phase = .failed
            askCard.status = "couldn't reply — \(error)"
            showAnswerCard(askCard)
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

    private struct Delta: Decodable { let type: String; let session: String?; let text: String?; let stopReason: String?; let message: String? }

    /// 129 §3.2: a connection that never sets a focus gets every session's deltas; ours wants the desk's only.
    /// Re-applied on every `PendingPrompts` (the first item of each (re)subscription); pre-129 engines refuse, fine.
    private func focusDeltas() {
        guard let s = ask.router.deskSession else { return }
        Task {
            do { try await client.setEventFocus([s]) } catch { return Log.write("set_event_focus failed: \(error)") }
            if AskFlags.probe != nil { AskLog.write("set_event_focus([\(s.prefix(8))…]) ok") }
        }
    }

    /// Every event, raw: `TextDelta`/`TurnFinished`/`Error` on the desk feed the reply while one is being collected
    /// (a failed or cancelled turn ends it with the reason); `Dictation*` (W6) feed the live dictation; `DictionarySuggestion`
    /// (153 §6.3) the card.
    func askObserve(_ data: Data) {
        guard let d = try? TheuthJSON.decoder.decode(Delta.self, from: data) else { return }
        if d.type == "PendingPrompts" { return focusDeltas() }
        if d.type.hasPrefix("Dictation") { return dictationObserve(data) }
        if d.type == "DictionarySuggestion" { return suggestionObserve(data) }
        guard let r = ask.router.reply, !r.finished, let session = d.session else { return }
        let event: ReplyEvent
        switch d.type {
        case "TextDelta": event = .textDelta(session: session, text: d.text ?? "")
        case "TurnFinished": event = .turnFinished(session: session, stopReason: d.stopReason ?? "")
        case "Error": event = .error(session: session, message: d.message ?? "")
        default: return
        }
        guard ask.router.observe(event), let reply = ask.router.reply else { return }
        // ponytail: one publish per delta; coalesce on a timer if the stream ever makes the panel stutter.
        let text = reply.receiptText
        if let id = ask.current { ask.receipts.update(id: id, text: text) }
        setReceipt(.reply(who: "theuth", text: text.isEmpty ? "…" : text, session: reply.session))
        // 153 §3.3: the same stream drives the answer card (`AnswerCardState` is C1's model; the view is theirs too).
        askCard.phase = reply.failure == nil ? .answer : .failed
        askCard.text = reply.text
        askCard.status = reply.failure.map { "couldn't reply — \($0)" } ?? ""
        askCard.turnFinished = reply.finished
        showAnswerCard(askCard)
        guard reply.finished else { return }
        expireReceipt(after: 12)
        // S3: the same reply the receipt row shows, spoken through the voice key's path once it is final; partials never.
        if SpokenReplies.shouldSpeak(enabled: askSpeakEnabled, reply: reply) { speaker.speak(reply.text, on: reply.session, from: .askBar) }
    }

    // MARK: live dictation (W6; design 54 §8) — decoded with `TheuthKit.DictationWire` (153 §14.4)

    /// Reduce one `Dictation*` event; `onDictation(dictation, takeText)` lets the Ask bar update its field.
    private func dictationObserve(_ data: Data) {
        onDictationWire?(String(decoding: data, as: UTF8.self))
        guard let w = try? TheuthJSON.decoder.decode(DictationWire.self, from: data), let e = AskDictation.event(w) else { return }
        let take = ask.dictation.handle(e)
        onDictation?(ask.dictation, take)
    }

    /// 153 §6.3: the engine proposes a dictionary entry after a correction; the card (`DictionarySuggestionCard`) shows
    /// `ask.suggestion` and answers through `answerSuggestion`. A newer suggestion replaces an unanswered one.
    private func suggestionObserve(_ data: Data) {
        guard let s = try? TheuthJSON.decoder.decode(DictionarySuggestion.self, from: data) else { return }
        ask.suggestion = s
        AskLog.write("dictionary suggestion \(s.id): \(s.heard) → \(s.typed)")
    }

    /// `dictionary_suggestion_answer(id, answer)`; `nil` = the 20 s auto-dismiss, which the engine counts itself (§6.4: no cooldown).
    func answerSuggestion(_ answer: SuggestionAnswer?) {
        guard let s = ask.suggestion else { return }
        ask.suggestion = nil
        guard let answer else { return }
        Task {
            do { try await client.dictionarySuggestionAnswer(id: s.id, answer: answer); AskLog.write("suggestion \(s.id) → \(answer.rawValue)") }
            catch { Log.write("dictionary_suggestion_answer failed: \(error)") }
        }
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
/// §3.3: the Ask bar's one answer card, fed by `commitAsk` and the reply stream; `Store.showAnswerCard` renders it (C1).
@MainActor private var askCard = CompanionCore.AnswerCardState()

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
    /// W6: with `--ask-probe`, stream a synthetic tone through the live dictation path.
    static let syntheticDictation = CommandLine.arguments.contains("--synthetic-dictation")
    /// 149s: with `--ask-probe`, the spoken-reply scenario (`AskProbe.speakScenario`); also turns Ask speech on for the run.
    static let probeSpeak = CommandLine.arguments.contains("--probe-speak")
    /// Any automated run: no hotkey, no microphone, no sends.
    static var automated: Bool { Flags.anyProbe || probe != nil }
}
