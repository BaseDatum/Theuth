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

    /// `send_input` went out: collect the desk's reply from now until its turn finishes.
    public mutating func sent(on session: String) { reply = AskReply(session: session) }

    /// Feed `TextDelta` / `TurnFinished` from the event stream; `true` when the receipt text changed.
    @discardableResult
    public mutating func observe(_ e: ReplyEvent) -> Bool {
        guard var r = reply, !r.finished else { return false }
        switch e {
        case .textDelta(let session, let text):
            guard session == r.session else { return false }
            r.text += text
        case .turnFinished(let session):
            // A turn that ended before anything streamed is the one we steered into, not the reply.
            guard session == r.session, !r.text.isEmpty else { return false }
            r.finished = true
        }
        reply = r
        return true
    }
}

public enum ReplyEvent: Equatable, Sendable {
    case textDelta(session: String, text: String)
    case turnFinished(session: String)
}

/// The desk's streamed answer; the receipt row shows `firstTwoLines`.
public struct AskReply: Equatable, Sendable {
    public let session: String
    public var text = ""
    public var finished = false

    public init(session: String) { self.session = session }

    /// The first two non-empty lines, markdown bullets and heading marks stripped, joined with " — ".
    public var firstTwoLines: String {
        let lines = text.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces).drop { "#-*>".contains($0) || $0 == " " } }
            .map { String($0).trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        return lines.prefix(2).joined(separator: " — ")
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
