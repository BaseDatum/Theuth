import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W1: per kind — header, status line, bubbles, options, footer verbs, `ifWrong` (§3.3, Revision 1 rows 2, 3, 6, 7).
struct CardAnatomyTests {
    private let now = Date(timeIntervalSince1970: 1_760_000_000)
    private func ms(_ secondsAgo: TimeInterval) -> UInt64 { UInt64((now.timeIntervalSince1970 - secondsAgo) * 1000) }

    private func ask(context: ItemContext? = nil, agent: String? = "w1", session: String? = "s-1", ago: TimeInterval = 120) -> AttentionItem {
        let q = Question(id: "q", question: "Which spacing?", options: [Choice(label: "12 pt"), Choice(label: "8 pt"), Choice(label: "one strip")], recommended: 0)
        return AttentionItem(id: "ask:1", kind: .ask, title: "Which spacing?", atMs: ms(ago), agent: agent, session: session,
                             ask: AskRequested(id: "ask-1", session: session ?? "", questions: [q]), context: context)
    }

    private func finished(ago: TimeInterval = 10) -> AttentionItem {
        AttentionItem(id: "finished:3", kind: .finished, title: "Island panel is in", atMs: ms(ago), agent: "w3", session: "s-3",
                      summary: "Drag, snap-back and position memory.", nextSteps: ["Merge it", "Show me the no-notch pill"])
    }

    private func info(_ id: String, _ state: AgentState, agent: String? = "omp", project: String? = nil, last: String = "") -> AgentInfo {
        AgentInfo(id: id, name: "n", agent: agent, state: state, last: last, project: project, activity: SessionActivity(state: state, live: true))
    }

    private func anatomy(_ item: AttentionItem, packet: Packet? = nil, approval: ApprovalRequested? = nil, activity: AgentInfo? = nil,
                         session: SessionSummary? = nil, position: Int = 2, count: Int = 3) -> CardAnatomy {
        CardAnatomy.make(card: Card.make(item: item, packet: packet, approval: approval), position: position, count: count,
                         glance: "3 need you · 5 working · 2 done", activity: activity, session: session, now: now)
    }

    @Test func headerCarriesPositionCountAndGlance() {
        let a = anatomy(ask(), position: 2, count: 3)
        #expect(a.header == CardAnatomy.Header(name: "w1", position: 2, count: 3, glance: "3 need you · 5 working · 2 done"))
        #expect(anatomy(ask(agent: nil, session: "0192c5e0-7a1b-7c2d")).header.name == "0192c5e0", "no agent: the short session, as Card.who")
    }

    @Test func statusLineSaysJustNowUnderFortyFiveSeconds() {
        #expect(anatomy(finished(ago: 10)).status.age == "just now")
        #expect(anatomy(finished(ago: 44.9)).status.age == "just now")
        #expect(anatomy(finished(ago: 45)).status.age == "45 s", "at 45 s the relative time takes over (RailModel.ago)")
        #expect(anatomy(ask(ago: 120)).status.age == "2 m")
        #expect(anatomy(finished()).status.state == "Finished")
        #expect(anatomy(ask()).status.state == "Waiting for your answer")
        #expect(anatomy(ask(), activity: info("s-1", .cancelled)).status.state == "Stopped", "§5.4 B: after the cancel the line says so")
    }

    @Test func statusLineProjectFallbackChain() {
        let s = SessionSummary(id: "s-1", title: "t", agent: "omp", cwd: "/Users/sky/Develop/theuth", project: "theuth-proj")
        #expect(anatomy(ask(), activity: info("s-1", .running, project: "from-activity"), session: s).status.project == "from-activity", "AgentInfo.project first")
        #expect(anatomy(ask(), activity: info("s-1", .running), session: s).status.project == "theuth-proj", "then SessionSummary.project")
        var noProject = s; noProject.project = nil
        #expect(anatomy(ask(), session: noProject).status.project == "theuth", "then cwd's last component")
        noProject.cwd = nil
        #expect(anatomy(ask(), session: noProject).status.project == "omp", "then the harness key as text (Revision 1 row 6)")
        noProject.agent = nil
        #expect(anatomy(ask(), session: noProject).status.project == nil)
        // The harness is the icon: nil = theuth, unknown keys = unknown (D18).
        #expect(anatomy(ask(), session: s).status.harness == "omp")
        #expect(anatomy(ask()).status.harness == "theuth")
        #expect(anatomy(ask(), activity: info("s-1", .running, agent: "aider")).status.harness == "unknown")
    }

    @Test func bubblesComeFromItemContextWhenPresent() {
        let ctx = ItemContext(request: "Bring the rail to One's pill layout", lastReply: "Three layouts are drafted.")
        let a = anatomy(ask(context: ctx), activity: info("s-1", .awaiting(from: "ask"), last: "ignored"), session: SessionSummary(id: "s-1", title: "ignored"))
        #expect(a.bubbles == [.init(side: .user, text: ctx.request!), .init(side: .agent, text: ctx.lastReply!)])
        let half = anatomy(ask(context: ItemContext(request: nil, lastReply: "only the reply")))
        #expect(half.bubbles == [.init(side: .agent, text: "only the reply")], "a missing half is simply absent")
        let long = anatomy(ask(context: ItemContext(request: String(repeating: "x", count: 500), lastReply: nil)))
        #expect(long.bubbles[0].text.count == 160 && long.bubbles[0].text.hasSuffix("…"), "≤ 160 chars")
    }

    @Test func bubblesFallBackToTheInterimPairWithoutContext() {
        let a = anatomy(ask(), activity: info("s-1", .awaiting(from: "ask"), last: "Drafted three layouts"), session: SessionSummary(id: "s-1", title: "Rail pills"))
        #expect(a.bubbles == [.init(side: .user, text: "Rail pills"), .init(side: .agent, text: "Drafted three layouts")], "D4 interim: title + last")
        #expect(anatomy(ask(), session: SessionSummary(id: "s-1", title: "")).bubbles.isEmpty, "empty strings are not bubbles")
        #expect(anatomy(ask()).bubbles.isEmpty)
    }

    @Test func optionsMarkRecommendedAndArrow() {
        let a = anatomy(ask())
        #expect(a.options.map(\.label) == ["12 pt", "8 pt", "one strip"])
        #expect(a.options.map(\.recommended) == [true, false, false], "the view renders \"(Recommended)\" and → from these")
        #expect(a.options.map(\.key) == [1, 2, 3])
        #expect(anatomy(finished()).options.map(\.label) == ["Merge it", "Show me the no-notch pill"], "Next steps are the finished card's options")
    }

    @Test func stopOnlyWhenTheAgentRunsOrWaits() {
        #expect(anatomy(ask(), activity: info("s-1", .awaiting(from: "ask"))).footer.contains(.stop), "waiting")
        #expect(anatomy(ask(), activity: info("s-1", .running)).footer.contains(.stop), "running")
        #expect(anatomy(ask(), activity: info("s-1", .approval(tool: "bash"))).footer.contains(.stop))
        #expect(!anatomy(ask(), activity: info("s-1", .idle)).footer.contains(.stop), "idle: nothing to stop")
        #expect(!anatomy(ask(), activity: info("s-1", .cancelled)).footer.contains(.stop))
        #expect(!anatomy(ask()).footer.contains(.stop), "no activity row: the state is unknown, never guessed")
        #expect(!anatomy(ask(session: nil), activity: info("s-1", .running)).footer.contains(.stop), "no session: nothing to cancel")
        let approval = AttentionItem(id: "approval:1", kind: .approval, title: "bash", atMs: ms(5), agent: "w2", session: "s-2", approval: "ap-1")
        #expect(anatomy(approval, activity: info("s-2", .approval(tool: "bash"))).footer.contains(.stop))
        let blocked = AttentionItem(id: "blocked:1", kind: .blocked, title: "needs a token", atMs: ms(5), task: "t-1", session: "s-4")
        #expect(anatomy(blocked, activity: info("s-4", .running)).footer.contains(.stop))
    }

    @Test func stopAbsentOnFinishedCards() {
        // Revision 1 row 2: a Finished item is a receipt of a clean end_turn — even with a running activity row it has no Stop.
        #expect(!anatomy(finished(), activity: info("s-3", .running)).footer.contains(.stop))
        #expect(!anatomy(finished(), activity: info("s-3", .idle)).footer.contains(.stop))
        let f = anatomy(finished(), activity: info("s-3", .idle))
        #expect(f.footer.contains(.clear) && f.footer.contains(.reply) && f.footer.contains(.point) && f.footer.contains(.legend))
        #expect(!f.footer.contains(.snooze), "a receipt is not snoozed")
    }

    @Test func clearOnlyWhenClearable() {
        #expect(anatomy(finished()).footer.contains(.clear))
        let show = AttentionItem(id: "show:1", kind: .show, title: "look", atMs: ms(5))
        #expect(anatomy(show).footer.contains(.clear))
        #expect(!anatomy(ask()).footer.contains(.clear))
        let approval = AttentionItem(id: "approval:1", kind: .approval, title: "bash", atMs: ms(5), session: "s-2", approval: "ap-1")
        #expect(!anatomy(approval).footer.contains(.clear))
        #expect(!anatomy(show).footer.contains(.reply) && !anatomy(show).footer.contains(.point), "nothing to reply to, no session")
    }

    @Test func askCardsSnoozeNotClear() {
        // Revision 1 row 3: "Snooze S", never "Clear E", on an ask.
        let a = anatomy(ask(), activity: info("s-1", .awaiting(from: "ask")))
        #expect(a.footer.contains(.snooze) && !a.footer.contains(.clear))
        #expect(a.footer == [.reply, .talk, .point, .stop, .snooze, .legend], "F5 order: reply row verbs, then the footer pills")
        let decision = AttentionItem(id: "decision:1", kind: .decision, title: "Pick", atMs: ms(5), task: "t", session: "s-5", decision: "d-1", packet: "pk-1")
        let d = anatomy(decision)
        #expect(d.footer.contains(.snooze) && !d.footer.contains(.clear))
    }

    @Test func ifWrongRenderedWhenPresent() {
        let decision = AttentionItem(id: "decision:1", kind: .decision, title: "Pick", atMs: ms(5), task: "t-1", session: "s-5", decision: "d-1", packet: "pk-1")
        let packet = Packet(key: "pk-1", task: "t-1", kind: .decision, question: "Which?", summary: "why", options: [PacketOption(label: "A"), PacketOption(label: "B")],
                            ifWrong: "A wrong merge ships a broken overlay")
        #expect(anatomy(decision, packet: packet).ifWrong == "A wrong merge ships a broken overlay")
        var empty = packet; empty.ifWrong = ""
        #expect(anatomy(decision, packet: empty).ifWrong == nil, "an empty line is not rendered")
        var absent = packet; absent.ifWrong = nil
        #expect(anatomy(decision, packet: absent).ifWrong == nil)
        #expect(anatomy(ask()).ifWrong == nil, "asks carry no packet")
    }
}
