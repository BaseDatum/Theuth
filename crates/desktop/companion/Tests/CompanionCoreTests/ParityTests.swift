import Testing
import Foundation
import TheuthKit
@testable import CompanionCore

// 130 PARITY: the Windows Core's W2Tests and the StoreModelTests the two Store defects came from (134 W1 §7.2),
// against the same hold → E5 re-read → send path, with a recording engine instead of a socket.

private let t0 = Date(timeIntervalSince1970: 1_791_448_800)

private func hire(_ name: String?) -> AttentionItem {
    AttentionItem(id: "hire:\(name ?? "?")", kind: .hire, title: "agent:theuth proposes hiring \(name ?? "?") (Docs)",
                  atMs: 1_791_448_000_000, task: "t-hire", agent: name)
}

private func blocked(_ task: String?, decision: String? = nil) -> AttentionItem {
    AttentionItem(id: "blocked:\(task ?? "?")", kind: .blocked, title: "waiting on the vendor", atMs: 1_791_448_000_000,
                  task: task, decision: decision)
}

/// Records every call; answers from settable fields (the C# `FakeEngine`).
private final class RecordingEngine: StoreEngine, @unchecked Sendable {
    var calls: [String] = []
    var items: [AttentionItem] = []
    var answerResult = true
    var fail: Error?

    private func record<T>(_ call: String, _ value: T) throws -> T {
        calls.append(call)
        if let fail { throw fail }
        return value
    }

    func attentionList() async throws -> [AttentionItem] { try record("attention_list", items) }
    func answerAsk(id: String, answers: [Answer]?) async throws -> Bool {
        try record("answer_ask \(id) \(answers.map { "\($0)" } ?? "null")", answerResult)
    }
    func resolveApproval(id: String, decision: Decision) async throws -> Bool {
        try record("resolve_approval \(id) \(decision.rawValue)", answerResult)
    }
    func taskDecide(decision: String, answer: String) async throws { try record("task_decide \(decision) \(answer)", ()) }
    func taskMove(id: String, state: String, note: String?) async throws { try record("task_move \(id) \(state) \(note ?? "null")", ()) }
    func sendInput(session: String, text: String) async throws -> String { try record("send_input \(session) \(text)", session) }
    func attentionDismiss(id: String) async throws { try record("attention_dismiss \(id)", ()) }
    func attentionSnooze(id: String, untilMs: UInt64) async throws { try record("attention_snooze \(id) \(untilMs)", ()) }
    func steer(session: String, text: String) async throws -> String { try record("steer \(session) \(text)", "input-1") }
    func agentApprove(name: String) async throws { try record("agent_approve \(name)", ()) }
    func agentRemove(name: String, note: String?) async throws { try record("agent_remove \(name) \(note ?? "null")", ()) }
    // 149 W1
    func cancel(session: String, scope: CancelScope?) async throws { try record("cancel \(session) \(scope?.rawValue ?? "null")", ()) }
    func sendInput(session: String?, text: String, files: [String], delivery: Delivery?, id: String?) async throws -> String {
        try record("send_input \(session ?? "null") \(text) files=\(files)", session ?? "")
    }
}

struct PerKindOptionTests {
    @Test func hireCardsOfferHireAndDeclineWithNoPrePick() {
        let c = Card.make(item: hire("docs"))
        #expect(c.options.map(\.label) == ["Hire", "Decline"])
        #expect(c.options.map(\.key) == [1, 2])
        #expect(c.recommended == nil, "P13: ⏎ is inert on a hire")
        #expect(c.options[0].action == .hireAgent(name: "docs"))
        #expect(c.options[1].action == .declineHire(name: "docs"))
        #expect(!c.clearable)
        #expect(c.who == "docs")
        #expect(Card.make(item: hire(nil)).options.isEmpty, "nothing to name: no options")
    }

    @Test func blockedCardsOfferUnblockUnlessTheBlockerIsADecision() {
        let c = Card.make(item: blocked("t-7"))
        #expect(c.options.count == 1)
        #expect(c.options.first?.label == "Unblock")
        #expect(c.options.first?.detail == "back to todo")
        #expect(c.options.first?.action == .unblock(task: "t-7"))
        #expect(c.recommended == nil)
        #expect(Card.make(item: blocked("t-7", decision: "d-1")).options.isEmpty, "a decision blocker is the decision kind")
        #expect(Card.make(item: blocked(nil)).options.isEmpty)
    }

    /// 130 §4.2 "Hire · Decline · Open": with real options the panel no longer synthesises an "Open in theuth" row,
    /// so Open must stay one keystroke away — O → `.openBoard` → `Store` opens `card.attentionURL` on every card.
    @Test func openStaysOneKeystrokeAwayOnHireAndBlockedCards() {
        #expect(KeyGrammar.intent(for: KeyPress(chars: "o")) == .openBoard)
        for card in [Card.make(item: hire("docs")), Card.make(item: blocked("t-7"))] {
            #expect(card.attentionURL.absoluteString == "theuth://attention/\(card.item.id)")
            #expect(!card.options.contains { $0.key == 0 }, "no option claims the O row's key")
            #expect(card.options.allSatisfy { (1...3).contains($0.key) })
        }
    }
}

struct CommitDispatchTests {
    private let engine = RecordingEngine()

    private func run(_ action: CardAction, item: String = "hire:docs") async -> Commit.Outcome {
        await Commit.run(itemId: item, action: action, engine: engine, now: t0)
    }

    @Test func optionOneOnAHireReReadsThenApproves() async {
        engine.items = [hire("docs")]
        let outcome = await run(.hireAgent(name: "docs"))
        #expect(engine.calls == ["attention_list", "agent_approve docs"], "E5: re-read first")
        #expect(outcome == .sent)
    }

    @Test func declineRemovesTheProposalAndUnblockMovesTheTaskToTodo() async {
        engine.items = [hire("docs"), blocked("t-7")]
        #expect(await run(.declineHire(name: "docs")) == .sent)
        #expect(engine.calls.contains("agent_remove docs null"))
        engine.calls = []
        #expect(await run(.unblock(task: "t-7"), item: "blocked:t-7") == .sent)
        #expect(engine.calls.contains("task_move t-7 todo null"))
    }

    @Test func aHireAnsweredElsewhereIsStaleAndNeverSent() async {
        engine.items = [] // approved in the main window during the hold
        let outcome = await run(.hireAgent(name: "docs"))
        #expect(outcome == .gone(latest: []))
        #expect(engine.calls == ["attention_list"])
    }

    @Test func anEngineWithoutTheW2CallsFailsTheCommitVisibly() async {
        engine.items = [hire("docs")]
        struct MethodNotFound: Error, CustomStringConvertible { var description: String { "rpc -32601: Method not found: agent_approve" } }
        engine.fail = MethodNotFound()
        let outcome = await run(.hireAgent(name: "docs"))
        guard case .failed(let why) = outcome else { Issue.record("\(outcome)"); return }
        #expect(why.contains("agent_approve"))
    }

    /// Every other kind's RPC, exactly as Store.commit sent it before the move (130 §4.2 table).
    @Test func everyActionMapsToItsRpc() async {
        let id = "ask:1"
        engine.items = [AttentionItem(id: id, kind: .ask, title: id, atMs: 1, session: "s-2")]
        let table: [(CardAction, [String])] = [
            (.answerAsk(askId: "ask-1", answers: [.choice(index: 1)]), ["answer_ask ask-1 \([Answer.choice(index: 1)])"]),
            (.decide(decision: "d-1", answer: "B"), ["task_decide d-1 B"]),
            (.approve(approvalId: "ap-1", decision: .allow), ["resolve_approval ap-1 allow"]),
            (.reviewApprove(task: "t-9"), ["task_move t-9 done null"]),
            (.reviewSendBackNote(task: "t-9", note: "add the restore check"), ["task_move t-9 todo add the restore check"]),
            (.sendNextStep(session: "s-2", text: "Open a PR"), ["send_input s-2 Open a PR", "attention_dismiss \(id)"]),
            (.dismiss(id: id), ["attention_dismiss \(id)"]),
            (.snooze(id: id), ["attention_snooze \(id) \(UInt64((t0.timeIntervalSince1970 + 1800) * 1000))"]),
            (.denyWithNote(approvalId: "ap-1", session: "s-2", note: "not that file"), ["resolve_approval ap-1 deny", "steer s-2 not that file"]),
        ]
        for (action, want) in table {
            engine.calls = []
            #expect(await run(action, item: id) == .sent, "\(action)")
            #expect(engine.calls == ["attention_list"] + want, "\(action)")
        }
        engine.calls = []
        #expect(await run(.reviewSendBack(task: "t-9"), item: id) == .skipped, "needs its note first")
        #expect(await run(.askTheuth(text: "x"), item: id) == .skipped, "the Ask bar's own path")
        #expect(engine.calls == ["attention_list", "attention_list"], "nothing sent")
        engine.answerResult = false
        engine.calls = []
        #expect(await run(.denyWithNote(approvalId: "ap-1", session: "s-2", note: "n"), item: id) == .stale)
        #expect(engine.calls == ["attention_list", "resolve_approval ap-1 deny"], "a refused deny carries no note")
        #expect(await run(.answerAsk(askId: "ask-1", answers: [.skipped]), item: id) == .stale, "the engine's own stale check")
    }
}

struct FetchGateTests {
    @Test func aRefetchAskedForDuringOneRunsOnceMoreAfterIt() {
        var gate = FetchGate()
        let first = gate.begin()
        let second = gate.begin()
        let third = gate.begin()
        #expect(first && !second && !third, "two requests mid-fetch coalesce")
        let again = gate.end()
        #expect(again, "exactly one more (Store.refetch's defer ordering lost it)")
        let followUp = gate.begin()
        let afterFollowUp = gate.end()
        #expect(followUp && !afterFollowUp)
        #expect(!gate.fetching)
        let idle = gate.begin()
        #expect(idle, "idle again")
    }
}
