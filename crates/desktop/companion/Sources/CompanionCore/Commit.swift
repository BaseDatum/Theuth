import Foundation
import TheuthKit

/// The engine calls a committed keystroke makes (the app: `RpcClient`; tests: a recorder).
public protocol StoreEngine: Sendable {
    func attentionList() async throws -> [AttentionItem]
    func answerAsk(id: String, answers: [Answer]?) async throws -> Bool
    func resolveApproval(id: String, decision: Decision) async throws -> Bool
    func taskDecide(decision: String, answer: String) async throws
    func taskMove(id: String, state: String, note: String?) async throws
    func sendInput(session: String, text: String) async throws -> String
    func attentionDismiss(id: String) async throws
    func attentionSnooze(id: String, untilMs: UInt64) async throws
    func steer(session: String, text: String) async throws -> String
    func agentApprove(name: String) async throws
    func agentRemove(name: String, note: String?) async throws
}

extension RpcClient: StoreEngine {}

/// E5: re-read, then send. One `CardAction` → exactly one RPC (two for the note-carrying ones), so the per-kind
/// table (130 §4.2) is checked here, not in the app.
public enum Commit {
    public enum Outcome: Equatable {
        /// Gone from the re-read list (answered elsewhere); `latest` is applied so the panel catches up.
        case gone(latest: [AttentionItem])
        case sent
        /// The engine said `false`: already answered elsewhere. Never retried.
        case stale
        case failed(String)
        /// Nothing to send: a send-back still collecting its note, or the Ask bar's own path.
        case skipped
    }

    public static func run(itemId: String, action: CardAction, engine: StoreEngine, now: Date) async -> Outcome {
        do {
            let latest = try await engine.attentionList()
            guard StaleGuard.allows(id: itemId, latest: latest) else { return .gone(latest: latest) }
            var ok = true
            switch action {
            case .answerAsk(let id, let answers): ok = try await engine.answerAsk(id: id, answers: answers)
            case .decide(let decision, let answer): try await engine.taskDecide(decision: decision, answer: answer)
            case .approve(let id, let decision): ok = try await engine.resolveApproval(id: id, decision: decision)
            case .reviewApprove(let task): try await engine.taskMove(id: task, state: "done", note: nil)
            case .reviewSendBackNote(let task, let note): try await engine.taskMove(id: task, state: "todo", note: note)
            case .sendNextStep(let session, let text):
                _ = try await engine.sendInput(session: session, text: text)
                try await engine.attentionDismiss(id: itemId)
            case .dismiss(let id): try await engine.attentionDismiss(id: id)
            case .snooze(let id):
                let until = UInt64((now.timeIntervalSince1970 + KeyGrammar.snoozeInterval) * 1000)
                try await engine.attentionSnooze(id: id, untilMs: until)
            case .denyWithNote(let id, let session, let note):
                ok = try await engine.resolveApproval(id: id, decision: .deny)
                if ok, let session { _ = try await engine.steer(session: session, text: note) }
            case .hireAgent(let name): try await engine.agentApprove(name: name)
            case .declineHire(let name): try await engine.agentRemove(name: name, note: nil)
            case .unblock(let task): try await engine.taskMove(id: task, state: "todo", note: nil)
            case .reviewSendBack, .askTheuth: return .skipped
            }
            return ok ? .sent : .stale
        } catch {
            return .failed("\(error)")
        }
    }
}

/// One refetch in flight at a time (130 §5.2); a request during it runs exactly once after it, so the panel never
/// stays stale. Pure — the caller runs the fetch.
public struct FetchGate: Equatable {
    public private(set) var fetching = false
    private var again = false

    public init() {}

    /// `true`: run the fetch now. `false`: one is in flight and will be followed by exactly one more.
    public mutating func begin() -> Bool {
        if fetching { again = true; return false }
        fetching = true
        return true
    }

    /// The fetch ended. `true` when a request came in meanwhile: call `begin()` again, it succeeds.
    public mutating func end() -> Bool {
        fetching = false
        defer { again = false }
        return again
    }
}
