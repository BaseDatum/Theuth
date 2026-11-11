import Foundation
import TheuthKit

/// `mesh_status` subset: this engine, the lease holder, peer names (`crates/mesh/src/types.rs:94-202`).
public struct MeshStatus: Codable, Equatable, Sendable {
    public struct Peer: Codable, Equatable, Sendable {
        public var id: String
        public var name: String
        public var reachable: Bool
        public init(id: String, name: String, reachable: Bool) { self.id = id; self.name = name; self.reachable = reachable }
    }
    public struct Lease: Codable, Equatable, Sendable {
        public var holder: String?
        public init(holder: String?) { self.holder = holder }
    }
    public var selfId: String
    public var selfName: String
    public var peers: [Peer]
    public var lease: Lease

    public init(selfId: String, selfName: String, peers: [Peer] = [], lease: Lease) {
        self.selfId = selfId; self.selfName = selfName; self.peers = peers; self.lease = lease
    }

    /// `desk_open` is answered only by the lease holder (`api.rs:1692`, `roster/src/lib.rs:492-494`).
    public var isHub: Bool { lease.holder == selfId }
    /// The hub's display name, or `nil` while no engine holds the lease.
    public var hubName: String? {
        guard let h = lease.holder else { return nil }
        return h == selfId ? selfName : (peers.first { $0.id == h }?.name ?? String(h.prefix(8)))
    }
}

/// Where a request goes (D7, R4): always theuth's desk, resolved once per session. No agent picker.
public struct AskRouter: Equatable, Sendable {
    public enum Desk: Equatable, Sendable {
        case unknown
        case ready(session: String, hub: Bool)
        /// Not reachable from here; the reason is shown in the receipt row.
        case unavailable(String)
    }

    public private(set) var desk: Desk = .unknown
    /// The reply being collected since the last send; `nil` until a send.
    public private(set) var reply: AskReply?

    public init() {}

    public var deskSession: String? { if case .ready(let s, _) = desk { return s } else { return nil } }

    /// `mesh_status` says whether `desk_open` may be called here. Off the hub the only hope is a replicated
    /// `agents_list` row carrying theuth's desk (`AgentSummary.desk`); else the request cannot go anywhere.
    public static func plan(mesh: MeshStatus?, agents: [AgentSummary]) -> Plan {
        guard let mesh, !mesh.isHub else { return .openHere } // no mesh: a single engine is its own hub
        if let s = agents.first(where: { $0.file.name == "theuth" })?.desk { return .use(session: s) }
        return .refuse("theuth's desk lives on \(mesh.hubName ?? "another engine") — open theuth there")
    }

    public enum Plan: Equatable, Sendable {
        case openHere
        case use(session: String)
        case refuse(String)
    }

    public mutating func resolved(session: String, hub: Bool) { desk = .ready(session: session, hub: hub) }
    public mutating func refused(_ reason: String) { desk = .unavailable(reason) }
    /// The desk closed or the connection was lost: resolve again on the next send.
    public mutating func forget() { desk = .unknown }

    /// `send_input` went out: collect the desk's reply from now until its turn finishes. `question` is the text as sent
    /// (the transcript's user row is matched against it, 189 D2).
    public mutating func sent(on session: String, question: String = "", now: Date = Date()) {
        reply = AskReply(session: session, question: question, now: now)
    }

    /// Feed one desk event; `true` when the reply changed. 189 D2: a normal `TurnFinished` no longer finishes the reply by
    /// itself — it marks `turnEnded` and the caller reads the transcript (`caughtUp`); D3: the terminal stop reasons end it
    /// at once as a failure. Any event for the desk re-arms the watchdog.
    @discardableResult
    public mutating func observe(_ e: ReplyEvent, now: Date = Date()) -> Bool {
        guard var r = reply, !r.finished, e.session == r.session else { return false }
        r.lastEventAt = now
        switch e {
        case .textDelta(_, let text):
            r.text += text
            r.deltas += 1
            if !text.isEmpty { r.thinking.end(now: now) } // 192 P3: the answer started
        case .reasoningDelta(_, let text):
            r.thinking.add(text, now: now)
            r.reasoningDeltas += 1
        case .reasoningDone(_, let ms):
            r.thinking.done(ms: ms)
        case .turnStarted(_, let turnSeq, let text):
            // A new turn on the desk: its own `TurnFinished` is still to come.
            r.turnEnded = false
            if r.text.isEmpty { r.thinking = Thinking() } // the turn we steered into ended its thinking; ours thinks anew
            if r.turnSeq == nil, let turnSeq, Self.same(text, r.question) { r.turnSeq = turnSeq }
        case .turnFinished(_, let stopReason):
            r.stopReason = stopReason
            r.thinking.end(now: now)
            if let why = AskReply.terminalFailure(stopReason) {
                r.failure = why
                r.finished = true
            } else {
                r.turnEnded = true
            }
        case .error(_, let message):
            // The engine's live `AgentEvent::Error` (e.g. a missing API key): the turn will not answer.
            r.failure = AskReply.clip(message)
            r.finished = true
            r.thinking.end(now: now)
        case .activity:
            reply = r
            return false
        }
        reply = r
        return true
    }

    /// 189 D2: `load_session` came back (`nil` = the read failed) for the reply `id`. Running: keep collecting (the
    /// transcript fills an empty stream once our own row is found). Not running: the transcript's text is authoritative
    /// and the reply finishes — after a `TurnFinished`, on the watchdog, or when our row and its answer are there (a
    /// reconnect or `Lagged` that lost the finish). After a `TurnFinished` it always finishes, with
    /// `AskReply.noReply` when there is no text at all. `true` when the reply changed.
    @discardableResult
    public mutating func caughtUp(id: UUID, loaded: Loaded?, trigger: CatchUpTrigger, now: Date) -> Bool {
        guard var r = reply, r.id == id, !r.finished else { return false }
        r.lastEventAt = now
        defer { reply = r }
        guard let loaded else {
            guard r.turnEnded else { return false }
            r.finish(text: nil, now: now)
            return true
        }
        let pick = TranscriptPick.reply(in: loaded.history, question: r.question, turnSeq: r.turnSeq)
        if loaded.running {
            r.turnEnded = false // the turn we saw end was the one we steered into; ours runs now
            guard r.text.isEmpty, pick.anchored, let t = pick.text else { return false }
            r.text = t
            r.thinking.end(now: now)
            return true
        }
        guard r.turnEnded || trigger == .watchdog || (pick.anchored && pick.text != nil) else { return false }
        r.finish(text: pick.text, now: now)
        return true
    }

    /// 189 D2: the watchdog's deadline passed with no desk event — answers whether to read the transcript, and re-arms.
    public mutating func watchdogFired(now: Date) -> Bool {
        guard var r = reply, !r.finished, let due = r.watchdogDeadline, now >= due else { return false }
        r.lastEventAt = now
        reply = r
        return true
    }

    /// Typed text and the stored row may differ by surrounding whitespace only.
    static func same(_ a: String, _ b: String) -> Bool {
        a.trimmingCharacters(in: .whitespacesAndNewlines) == b.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

public enum ReplyEvent: Equatable, Sendable {
    case textDelta(session: String, text: String)
    /// `TurnStarted` (`turn_seq` is the seq of the user row that opened it; `text` its echo).
    case turnStarted(session: String, turnSeq: Int64?, text: String)
    /// `stop_reason`: the provider's, or the engine's own (`crates/agent/src/loop_.rs`: cancelled · error · suspended ·
    /// timed_out · interrupted).
    case turnFinished(session: String, stopReason: String)
    case error(session: String, message: String)
    /// 192 P3: `ReasoningDelta` (`crates/agent/src/lib.rs` `{session, turn, text}`).
    case reasoningDelta(session: String, text: String)
    /// 192 P3: `ReasoningDone` (`{session, turn, ms}`): ms from the first reasoning chunk to the call's first other event.
    case reasoningDone(session: String, ms: UInt64)
    /// Any other event on a session: only re-arms the watchdog.
    case activity(session: String)

    public var session: String {
        switch self {
        case .textDelta(let s, _), .turnStarted(let s, _, _), .turnFinished(let s, _), .error(let s, _), .activity(let s),
             .reasoningDelta(let s, _), .reasoningDone(let s, _): return s
        }
    }
}

/// What made the reply read the transcript (189 D2), as logged.
public enum CatchUpTrigger: String, Equatable, Sendable {
    case turnFinished = "turn finished", lagged, reconnect, watchdog
}

/// The desk's streamed answer; the receipt row shows `receiptText`.
public struct AskReply: Equatable, Sendable {
    /// 189 D2: this long without a desk event → read the transcript.
    public static let watchdog: TimeInterval = 15
    /// 189 D2: the transcript rows a catch-up reads.
    public static let catchUpLimit: UInt32 = 8
    /// 189 D2: the turn finished and neither the stream nor the transcript holds any assistant text.
    public static let noReply = "theuth finished without a reply"

    /// One send; a catch-up for an earlier send never lands on a later one.
    public let id = UUID()
    public let session: String
    /// The text as sent; the transcript's user row is found by it.
    public var question = ""
    public var text = ""
    public var finished = false
    /// Set when the desk's turn failed or was cancelled instead of answering; `finished` is set with it.
    public var failure: String?
    /// The last `TurnFinished.stop_reason` seen.
    public var stopReason: String?
    /// `TurnStarted.turn_seq` of the turn our text opened; matches the transcript's `turn`.
    public var turnSeq: Int64?
    /// A normal `TurnFinished` arrived: the transcript decides the text, then the reply finishes.
    public var turnEnded = false
    /// `TextDelta`s collected (189 D6 logs the first).
    public var deltas = 0
    /// `ReasoningDelta`s collected (192 P4 logs the first).
    public var reasoningDeltas = 0
    /// 192 P3: the reasoning streamed so far; the card shows it while live, then "Thought for N s".
    public var thinking = Thinking()
    /// The last desk event (or catch-up): the watchdog runs from here.
    public var lastEventAt = Date.distantPast

    public init(session: String, question: String = "", now: Date = Date()) {
        self.session = session; self.question = question; lastEventAt = now
    }

    /// When the watchdog fires, while still collecting.
    public var watchdogDeadline: Date? { finished ? nil : lastEventAt.addingTimeInterval(Self.watchdog) }

    /// The status line for a failure: "couldn't reply — ‹why›", or `noReply` as is.
    public var failureLine: String? { failure.map { $0 == Self.noReply ? $0 : "couldn't reply — \($0)" } }

    /// What the receipt row shows after "theuth · ": the answer's first two lines, or why there is none.
    public var receiptText: String { failureLine ?? firstTwoLines }

    /// 189 F5: the first two lines worth reading, one per line — heading lines, code fences and marker-only lines (rules)
    /// dropped, quote and list markers stripped with the item text kept, inline marks rendered away (`**`, backticks, links).
    public var firstTwoLines: String {
        var out: [String] = []
        for raw in text.split(whereSeparator: \.isNewline) where !MarkdownLines.markersOnly(raw) {
            guard let content = MarkdownLines.content(raw) else { continue }
            let plain = MarkdownLines.plain(content)
            if !plain.isEmpty { out.append(plain) }
            if out.count == 2 { break }
        }
        return out.joined(separator: "\n")
    }

    /// The transcript's text replaces the streamed text (it is authoritative); no text anywhere is `noReply`.
    mutating func finish(text transcript: String?, now: Date) {
        if let t = transcript, !t.isEmpty { text = t }
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { failure = Self.noReply }
        finished = true
        thinking.end(now: now)
    }

    /// 189 D3: the stop reasons that mean no reply is coming, as the failure they read; `nil` = a normal finish.
    /// The Windows companion's set (`AskRouter.cs` `IsFailure`) plus `suspended`.
    public static func terminalFailure(_ stopReason: String) -> String? {
        switch stopReason {
        case "error": return "the turn failed"
        case "cancelled": return "the turn was cancelled"
        case "suspended": return "the turn was suspended"
        case "timed_out": return "the turn timed out"
        case "interrupted": return "the turn was interrupted"
        default: return nil
        }
    }

    /// An error message's first line, at most 80 characters.
    static func clip(_ message: String) -> String {
        let line = message.split(whereSeparator: \.isNewline).first.map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
        guard !line.isEmpty else { return "the turn failed" }
        return line.count > 80 ? String(line.prefix(79)) + "…" : line
    }
}

/// 189 F5/F7: line-level markdown for the plain-text preview and the streaming hold-back.
public enum MarkdownLines {
    /// 189 F7: `text` without a trailing line made only of markers and spaces (`#`, `## `, `- `, `1.`, `> `). The markdown
    /// library lays such a line out as an empty block and keeps it 0 pt tall when its text arrives in place, so while a
    /// reply streams the block first appears with real text. A finished reply is shown whole.
    public static func holdBack(_ text: String) -> String {
        let start = text.lastIndex(of: "\n").map { text.index(after: $0) } ?? text.startIndex
        return markersOnly(text[start...]) ? String(text[..<start]) : text
    }

    /// Only `#`, `-`, `*`, `+`, `>`, digits followed by `.` or `)`, and spaces; a blank line is not.
    static func markersOnly(_ line: Substring) -> Bool {
        let s = line.filter { $0 != " " && $0 != "\t" }
        guard !s.isEmpty else { return false }
        var i = s.startIndex
        while i < s.endIndex {
            if "#-*+>".contains(s[i]) { i = s.index(after: i); continue }
            guard s[i].isASCII, s[i].isNumber else { return false }
            let j = s[i...].firstIndex { !($0.isASCII && $0.isNumber) } ?? s.endIndex
            guard j < s.endIndex, s[j] == "." || s[j] == ")" else { return false }
            i = s.index(after: j)
        }
        return true
    }

    /// The line's text after its block markers (quote `>`s, then one list marker); `nil` for a heading or a code fence.
    static func content(_ line: Substring) -> Substring? {
        var s = line.drop { $0 == " " || $0 == "\t" }
        if s.hasPrefix("#") || s.hasPrefix("```") || s.hasPrefix("~~~") { return nil }
        while s.first == ">" { s = s.dropFirst().drop { $0 == " " } }
        if let n = listMarker(s) { s = s.dropFirst(n).drop { $0 == " " } }
        return s
    }

    /// The length of a list marker (`-`, `*`, `+`, `1.`, `1)`) at the start of `s`, followed by a space or the end.
    static func listMarker(_ s: Substring) -> Int? {
        var n: Int
        if let c = s.first, "-*+".contains(c) {
            n = 1
        } else {
            n = s.prefix { $0.isASCII && $0.isNumber }.count
            guard n > 0, n < s.count, ".)".contains(s[s.index(s.startIndex, offsetBy: n)]) else { return nil }
            n += 1
        }
        let rest = s.dropFirst(n)
        return rest.isEmpty || rest.first == " " || rest.first == "\t" ? n : nil
    }

    /// Inline markdown rendered to its characters (Foundation's parser), trimmed.
    static func plain(_ s: Substring) -> String {
        let a = try? AttributedString(markdown: String(s), options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))
        return (a.map { String($0.characters) } ?? String(s)).trimmingCharacters(in: .whitespaces)
    }
}

/// 189 D2: the ask's answer in a `load_session` page — the last assistant row after the ask's own user row. The row is
/// found by `turnSeq` (from `TurnStarted`), else as the latest user row whose text is the question; when that row opens
/// a turn, only that turn's assistant rows count (the desk interleaves peer messages and job results as turns of their
/// own). With neither (`anchored == false`) the newest assistant row stands in.
public enum TranscriptPick {
    public struct Pick: Equatable, Sendable {
        /// `nil`: no assistant text after the anchor (or anywhere, unanchored).
        public var text: String?
        /// The ask's own user row was found.
        public var anchored: Bool
    }

    public static func reply(in history: [HistoryEntry], question: String, turnSeq: Int64?) -> Pick {
        let users = history.indices.filter { history[$0].role == "user" }
        let anchor = turnSeq.flatMap { seq in users.last { history[$0].turn == seq } }
            ?? (question.isEmpty ? nil : users.last { AskRouter.same(history[$0].text ?? "", question) })
        let turn = anchor.flatMap { history[$0].turn }
        let text = history[(anchor.map { $0 + 1 } ?? 0)...]
            .last { $0.role == "assistant" && !($0.text ?? "").isEmpty && (turn == nil || $0.turn == turn) }?.text
        return Pick(text: text, anchored: anchor != nil)
    }
}

/// Everything W3 keeps on the store: routing, the Done zone, the R field. One `@Published` value.
public struct AskModel: Equatable, Sendable {
    public var router = AskRouter()
    public var receipts = ReceiptModel()
    /// The item whose card shows the R field.
    public var replyFor: String?
    /// The receipt of the ask in flight; its text follows the desk's stream.
    public var current: String?
    /// W6: the live dictation feeding the Ask bar's field.
    public var dictation = AskDictation()
    /// 153 §6.3: the unanswered `DictionarySuggestion` the card shows; `nil` once answered or dismissed.
    public var suggestion: DictionarySuggestion?
    public init() {}
}

public extension Card {
    /// R, reply in words: the kind's own path (brief item 5). Ask → a free-text `Answer` (`crates/tools/src/ask.rs`,
    /// one question only — W1 sends multi-question asks to theuth); decision → `task_decide(decision, text)`;
    /// approval → `resolve_approval(Deny)` + `steer(session, note)` (R1-5); review → `task_move(id, "todo", note)`;
    /// finished → `send_input(session, text)`. `nil` when the kind takes no words.
    func replyAction(_ text: String) -> CardAction? {
        switch item.kind {
        case .ask:
            guard let ask = item.ask, ask.questions.count == 1 else { return nil }
            return .answerAsk(askId: ask.id, answers: [.text(text)])
        case .decision: return item.decision.map { .decide(decision: $0, answer: text) }
        case .approval: return item.approval.map { .denyWithNote(approvalId: $0, session: item.session, note: text) }
        case .review: return item.task.map { .reviewSendBackNote(task: $0, note: text) }
        case .finished: return item.session.map { .sendNextStep(session: $0, text: text) }
        default: return nil
        }
    }

    /// What R shows above the field.
    var replyLabel: String {
        switch item.kind {
        case .approval: return "Deny with a note (R1-5: a reply denies)"
        case .review: return "Send back with a note"
        case .finished: return "Tell \(who) what next"
        default: return "Reply to \(who)"
        }
    }
}
