import Testing
import Foundation
import TheuthKit
@testable import CompanionCore

private let t0 = Date(timeIntervalSince1970: 1_791_448_800)
private func ms(_ secondsAgo: Double) -> UInt64 { UInt64((t0.timeIntervalSince1970 - secondsAgo) * 1000) }
private func item(_ id: String, _ kind: AttentionKind, agent: String? = nil, ago: Double = 60, session: String? = nil,
                  task: String? = nil, waitingSinceAgo: Double? = nil) -> AttentionItem {
    AttentionItem(id: id, kind: kind, title: id, atMs: ms(ago), task: task, agent: agent, session: session,
                  approval: kind == .approval ? "ap-\(id)" : nil, decision: kind == .decision ? "decision:\(id)" : nil,
                  waitingSinceMs: waitingSinceAgo.map(ms))
}
private func desk(_ name: String, _ status: AgentStatus, session: String? = nil) -> AgentSummary {
    AgentSummary(file: AgentFile(name: name, title: name, model: "m"), status: status, desk: session)
}
private func root(_ id: String, _ state: AgentState, live: Bool = true) -> AgentInfo {
    AgentInfo(id: id, name: "", model: "claude", state: state,
              activity: SessionActivity(state: state, turnStartedAt: state == .running ? t0.addingTimeInterval(-120) : nil, live: live))
}

struct RailModelTests {
    @Test func twoZonesAndOrdering() {
        let items = [
            item("approval:new", .approval, agent: "w3", ago: 40),
            item("review:old", .review, agent: "ops", ago: 660),
            item("decision:mid", .decision, agent: "w2", ago: 240),
            item("show:x", .show, ago: 10),
            item("finished:1", .finished, agent: "w1", ago: 360),
            // E2: `waiting_since_ms` wins over `at_ms`.
            item("ask:e2", .ask, agent: "w5", ago: 5, waitingSinceAgo: 900),
        ]
        let agents = [desk("w2", .working, session: "s2"), desk("ops", .paused), desk("dead", .unreachable),
                      desk("idle-desk", .idle), desk("newbie", .proposed)]
        let activity = [root("s2", .running), root("s7", .running), root("s8", .error),
                        root("cold", .running, live: false), root("s10", .approval(tool: "shell"))]
        let rail = RailModel.derive(items: items, agents: agents, activity: activity, now: t0)

        #expect(rail.needsYou.map(\.id) == ["ask:e2", "review:old", "decision:mid", "approval:new"], "oldest first; show is not needs-you")
        #expect(rail.needsYou.map(\.state) == Array(repeating: MarkState.needsYou(.ask), count: 1) + [.needsYou(.review), .needsYou(.decision), .needsYou(.approval)])
        // Zone 2: working · idle · finished · error · lapsed; the desk's own session (s2) is not doubled; proposed skipped; cold skipped.
        #expect(rail.agents.map(\.id) == ["agent:w2", "session:s7", "agent:idle-desk", "session:s10", "finished:1", "agent:dead", "session:s8", "agent:ops"])
        #expect(rail.agents.map(\.state) == [.working, .working, .idle, .idle, .finished, .error, .error, .lapsed])
        #expect(rail.needsYouCount == 4)
        #expect(rail.workingCount == 2)
        #expect(!(rail.quiet))
        #expect(rail.needsYou[0].detail == "w5, ask, 15 m")
        #expect(rail.agents[1].detail == "claude, working, running 2 m")
        #expect(rail.overflow == 0, "exactly 12 marks fit")
        #expect(rail.marks.count == 12)
        #expect(rail.marks.last?.id == "agent:ops")
    }

    @Test func quietAndOverflowKeepsNeedsYou() {
        let quiet = RailModel.derive(items: [item("show:1", .show)], agents: [desk("a", .idle)], activity: [], now: t0)
        #expect(quiet.quiet)
        #expect(quiet.marks.count == 1)
        let many = (0..<15).map { item("ask:\($0)", .ask, ago: Double($0)) }
        let rail = RailModel.derive(items: many, agents: [desk("a", .working)], activity: [], now: t0)
        #expect(rail.needsYou.count == 12)
        #expect(rail.agents.count == 0)
        #expect(rail.overflow == 4)
        #expect(rail.needsYouCount == 15)
        #expect(rail.workingCount == 1, "counted even when its mark is cut")
    }

    @Test func ago() {
        #expect(RailModel.ago(t0.addingTimeInterval(-59), t0) == "59 s")
        #expect(RailModel.ago(t0.addingTimeInterval(-60), t0) == "1 m")
        #expect(RailModel.ago(t0.addingTimeInterval(-7200), t0) == "2 h")
        #expect(RailModel.ago(t0.addingTimeInterval(-90000), t0) == "1 d")
        #expect(RailModel.ago(t0.addingTimeInterval(5), t0) == "0 s")
    }
}

struct PanelModelTests {
    let items = [item("finished:a", .finished, ago: 999), item("ask:b", .ask, ago: 30), item("show:c", .show, ago: 5),
                 item("review:d", .review, ago: 600), item("finished:e", .finished, ago: 100)]

    @Test func orderHeadlinePosition() {
        var p = PanelModel(items: items, workingCount: 5)
        #expect(p.items.map(\.id) == ["review:d", "ask:b", "finished:a", "finished:e", "show:c"])
        #expect(p.headline == "2 need you · 5 working · 2 done")
        #expect(p.position == "1 of 5")
        #expect(p.focused?.id == "review:d")
        #expect(p.others.map(\.id) == ["ask:b", "finished:a", "finished:e", "show:c"])
        p.move(1); p.move(1)
        #expect(p.position == "3 of 5")
        p.move(-10)
        #expect(p.focus == 0)
        p.move(10)
        #expect(p.focus == 4)
        p.focus(id: "ask:b")
        #expect(p.focus == 1)
        p.focus(id: "nope")
        #expect(p.focus == 1)
    }

    @Test func headlineVariants() {
        #expect(PanelModel(items: [item("ask:1", .ask)], workingCount: 0).headline == "1 needs you")
        #expect(PanelModel(items: [], workingCount: 2).headline == "2 working")
        #expect(PanelModel(items: [item("finished:1", .finished)], workingCount: 0).headline == "Nothing needs you · 1 done")
        #expect(PanelModel(items: [], workingCount: 0).headline == "Nothing needs you")
        #expect(PanelModel(items: [], workingCount: 0).position == "")
    }

    @Test func updateKeepsFocusByIdElseClamps() {
        var p = PanelModel(items: items)
        p.focus(id: "finished:e")
        p.update(items: items.filter { $0.id != "review:d" })
        #expect(p.focused?.id == "finished:e")
        p.update(items: [items[1]])
        #expect(p.focus == 0)
        #expect(p.focused?.id == "ask:b")
        p.update(items: [])
        #expect(p.focused == nil)
        #expect(p.position == "")
    }
}

struct UndoQueueTests {
    @Test func holdTickCancel() {
        var q = UndoQueue<String>()
        #expect(q.delay == 3)
        let first = q.tick(now: t0)
        #expect(first == nil)
        let none = q.hold("a", now: t0)
        #expect(none == nil)
        #expect(abs(q.remaining(now: t0.addingTimeInterval(1)) - 2) < 0.001)
        let early = q.tick(now: t0.addingTimeInterval(2.999))
        #expect(early == nil)
        let fired = q.tick(now: t0.addingTimeInterval(3))
        #expect(fired == "a")
        let again = q.tick(now: t0.addingTimeInterval(4))
        #expect(again == nil, "fires once")
        #expect(q.remaining(now: t0) == 0)
        q.hold("b", now: t0)
        let cancelled = q.cancel()
        #expect(cancelled)
        let cancelledTwice = q.cancel()
        #expect(!cancelledTwice)
        let afterCancel = q.tick(now: t0.addingTimeInterval(10))
        #expect(afterCancel == nil)
    }

    @Test func displacementAndClamp() {
        var q = UndoQueue<String>(delay: 9)
        #expect(q.delay == 5)
        q.hold("first", now: t0)
        let displaced = q.hold("second", now: t0.addingTimeInterval(1))
        #expect(displaced == "first", "a second keystroke commits the first")
        #expect(q.fireAt == t0.addingTimeInterval(6))
        var zero = UndoQueue<String>(delay: -1)
        #expect(zero.delay == 0)
        zero.hold("now", now: t0)
        let immediate = zero.tick(now: t0)
        #expect(immediate == "now")
    }
}

struct KeyGrammarTests {
    @Test func table() {
        let cases: [(KeyPress, Intent)] = [
            (KeyPress(chars: "1"), .answer(1)), (KeyPress(chars: "2"), .answer(2)), (KeyPress(chars: "3"), .answer(3)),
            (KeyPress(chars: "4"), .none),
            (KeyPress(chars: "\r", keyCode: 36), .answerRecommended), (KeyPress(chars: "\u{3}", keyCode: 76), .answerRecommended),
            (KeyPress(chars: "\u{1b}", keyCode: 53), .escape),
            (KeyPress(chars: "j"), .next), (KeyPress(chars: "K"), .previous),
            (KeyPress(chars: "", keyCode: 125), .next), (KeyPress(chars: "", keyCode: 126), .previous),
            (KeyPress(chars: "s"), .snooze), (KeyPress(chars: "e"), .clear),
            (KeyPress(chars: "d"), .openDiff), (KeyPress(chars: "o"), .openBoard), (KeyPress(chars: "o", command: true), .openSession),
            (KeyPress(chars: "1", command: true), .none), (KeyPress(chars: "r"), .reply), (KeyPress(chars: "l", command: true), .askBar),
        ]
        for (k, want) in cases { #expect(KeyGrammar.intent(for: k) == want, "\(k)") }
        #expect(KeyGrammar.snoozeInterval == 1800)
    }

    @Test func deepLinksAndStaleGuard() {
        #expect(DeepLink.attention("decision:task 7:d").absoluteString == "theuth://attention/decision:task%207:d")
        #expect(DeepLink.session("abc").absoluteString == "theuth://c/abc")
        let latest = [item("ask:1", .ask)]
        #expect(StaleGuard.allows(id: "ask:1", latest: latest))
        #expect(!(StaleGuard.allows(id: "ask:2", latest: latest)))
    }
}

struct CardModelTests {
    @Test func askCapsAtThreeAndPrePicks() {
        let q = Question(id: "q", question: "Which?", options: (0..<5).map { Choice(label: "o\($0)") }, recommended: 2)
        var it = item("ask:1", .ask, agent: "w2", session: "s")
        it.ask = AskRequested(id: "ask-id", session: "s", questions: [q])
        let c = Card.make(item: it)
        #expect(c.who == "w2")
        #expect(c.question == "Which?")
        #expect(c.options.map(\.key) == [1, 2, 3])
        #expect(c.moreOptions == 2)
        #expect(c.recommended?.label == "o2")
        #expect(c.options[0].action == .answerAsk(askId: "ask-id", answers: [.choice(index: 0)]))
        #expect(c.sessionURL?.absoluteString == "theuth://c/s")
        // Several questions: no keystroke answers, open in theuth instead.
        it.ask = AskRequested(id: "ask-id", session: "s", questions: [q, q])
        let multi = Card.make(item: it)
        #expect(multi.options.isEmpty)
        #expect(multi.why == "2 questions — answer them in theuth")
    }

    @Test func decisionUsesPacketAndForwardedRecommendation() {
        let it = item("decision:1", .decision, agent: "w2", task: "t")
        #expect(Card.make(item: it).options.isEmpty, "no packet yet")
        let p = Packet(key: "decision:1", task: "t", kind: .decision, question: "Layout?", summary: "Blocks checkout.",
                       options: [PacketOption(label: "A"), PacketOption(label: "B"), PacketOption(label: "C"), PacketOption(label: "D")],
                       recommended: 0, recommendation: Recommendation(text: "B.", option: 1))
        let c = Card.make(item: it, packet: p)
        #expect(c.question == "Layout?")
        #expect(c.why == "Blocks checkout.")
        #expect(c.recommended?.label == "B")
        #expect(c.options[2].action == .decide(decision: "decision:decision:1", answer: "C"))
        #expect(c.moreOptions == 1)
    }

    @Test func approvalForcingFunction() {
        let it = item("approval:1", .approval, agent: "w3")
        let mutating = ApprovalRequested(id: "ap-approval:1", session: "s", tool: "shell", args: .object(["command": .string("git push")]), risk: .mutating)
        let c = Card.make(item: it, approval: mutating)
        #expect(c.options.map(\.label) == ["Allow", "Deny"])
        #expect(c.recommended?.label == "Allow")
        #expect(c.why == "shell · mutating · git push")
        #expect(c.options[1].action == .approve(approvalId: "ap-approval:1", decision: .deny))
        var destructive = mutating
        destructive.risk = .destructive
        #expect(Card.make(item: it, approval: destructive).recommended == nil, "P13: no pre-pick")
        #expect(Card.make(item: it).recommended?.label == "Allow", "without the request we still offer Allow/Deny")
        #expect(Card.make(item: it).options.count == 2)
    }

    @Test func reviewChipsAndFailingChecks() {
        let it = item("review:1", .review, agent: "ops", task: "task-9")
        let good = Packet(key: "review:task-9", task: "task-9", kind: .review, summary: "Done.", checks: [Check(text: "tests", done: true)])
        let c = Card.make(item: it, packet: good)
        #expect(c.recommended?.label == "Approve")
        #expect(c.chips == [Chip(text: "tests", ok: true)])
        #expect(c.options[0].action == .reviewApprove(task: "task-9"))
        #expect(c.options[1].action == .reviewSendBack(task: "task-9"))
        #expect(c.options[1].action.needsNote)
        var bad = good
        bad.checks.append(Check(text: "restore", done: false))
        #expect(Card.make(item: it, packet: bad).recommended == nil, "P13: failing checks remove the pre-pick")
    }

    @Test func finishedAndOtherKinds() {
        var fin = item("finished:1", .finished, agent: "w1", session: "s4")
        fin.summary = "Landed."
        fin.nextSteps = ["Open a PR", "Fix the CLI", "third is dropped"]
        let c = Card.make(item: fin)
        #expect(c.why == "Landed.")
        #expect(c.clearable)
        #expect(c.options.map(\.label) == ["Open a PR", "Fix the CLI"])
        #expect(c.recommended?.action == .sendNextStep(session: "s4", text: "Open a PR"))
        let hire = Card.make(item: item("hire:x", .hire, agent: "x"))
        #expect(hire.options.isEmpty)
        #expect(!(hire.clearable))
        #expect(hire.attentionURL.absoluteString == "theuth://attention/hire:x")
        #expect(Card.make(item: item("show:1", .show)).clearable)
        #expect(Card.make(item: item("x", .other("weather"), session: "0192c5e0-7a1b")).who == "0192c5e0")
    }
}
