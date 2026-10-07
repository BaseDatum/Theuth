import Testing
import Foundation
import TheuthKit
@testable import CompanionCore

private let t0 = Date(timeIntervalSince1970: 1_791_448_800)
private func summary(_ name: String, desk: String? = nil) -> AgentSummary {
    AgentSummary(file: AgentFile(name: name, title: name, model: "m"), status: .idle, desk: desk)
}
private func item(_ id: String, _ kind: AttentionKind, session: String? = "s-1", task: String? = nil,
                  decision: String? = nil, approval: String? = nil) -> AttentionItem {
    AttentionItem(id: id, kind: kind, title: id, atMs: 0, task: task, agent: "w3", session: session,
                  approval: approval, decision: decision)
}

struct AskRouterTests {
    @Test func hubRuleFollowsTheLease() {
        let hub = MeshStatus(selfId: "e1", selfName: "mac", peers: [.init(id: "e2", name: "nuc", reachable: true)], lease: .init(holder: "e1"))
        let spoke = MeshStatus(selfId: "e1", selfName: "mac", peers: [.init(id: "e2", name: "nuc", reachable: true)], lease: .init(holder: "e2"))
        let none = MeshStatus(selfId: "e1", selfName: "mac", lease: .init(holder: nil))
        #expect(hub.isHub && hub.hubName == "mac")
        #expect(!spoke.isHub && spoke.hubName == "nuc")
        #expect(!none.isHub && none.hubName == nil)
    }

    @Test func planOpensHereOnTheHubAndRefusesElsewhere() {
        let spoke = MeshStatus(selfId: "e1", selfName: "mac", peers: [.init(id: "e2", name: "nuc", reachable: true)], lease: .init(holder: "e2"))
        #expect(AskRouter.plan(mesh: nil, agents: []) == .openHere, "no mesh: a lone engine is its own hub")
        #expect(AskRouter.plan(mesh: MeshStatus(selfId: "e1", selfName: "mac", lease: .init(holder: "e1")), agents: []) == .openHere)
        #expect(AskRouter.plan(mesh: spoke, agents: [summary("theuth", desk: "desk-9")]) == .use(session: "desk-9"),
                "off the hub a replicated desk id still routes; never another agent's desk")
        #expect(AskRouter.plan(mesh: spoke, agents: [summary("w2", desk: "desk-2")]) == .refuse("theuth's desk lives on nuc — open theuth there"))
    }

    @Test func replyCollectsOnlyTheDeskUntilItsTurnEnds() {
        var r = AskRouter()
        r.resolved(session: "desk", hub: true)
        #expect(r.observe(.textDelta(session: "desk", text: "early")) == false, "nothing collected before a send")
        r.sent(on: "desk")
        #expect(r.observe(.turnFinished(session: "desk")) == false, "an earlier turn ending with nothing streamed is not the reply")
        #expect(r.observe(.textDelta(session: "other", text: "noise")) == false)
        #expect(r.observe(.textDelta(session: "desk", text: "- handed to w3 — it owns crates/rpc\n")) == true)
        #expect(r.observe(.textDelta(session: "desk", text: "told w2 to wait\nthird line")) == true)
        #expect(r.reply?.firstTwoLines == "handed to w3 — it owns crates/rpc — told w2 to wait")
        #expect(r.observe(.turnFinished(session: "desk")) == true)
        #expect(r.reply?.finished == true)
        #expect(r.observe(.textDelta(session: "desk", text: "late")) == false, "finished replies stay put")
        #expect(r.reply?.text.contains("late") == false)
    }

    @Test func replyActionsFollowTheKind() {
        let ask = AskRequested(id: "ask-1", session: "s-1", questions: [Question(id: "q", question: "Which?")])
        var askItem = item("ask:1", .ask); askItem.ask = ask
        #expect(Card.make(item: askItem).replyAction("neither") == .answerAsk(askId: "ask-1", answers: [.text("neither")]))
        var multi = askItem; multi.ask = AskRequested(id: "ask-2", session: "s-1", questions: [Question(id: "a", question: "A"), Question(id: "b", question: "B")])
        #expect(Card.make(item: multi).replyAction("x") == nil, "multi-question asks are answered in theuth")
        #expect(Card.make(item: item("d", .decision, decision: "decision:7")).replyAction("ship B") == .decide(decision: "decision:7", answer: "ship B"))
        #expect(Card.make(item: item("a", .approval, approval: "ap-1")).replyAction("not that file")
                == .denyWithNote(approvalId: "ap-1", session: "s-1", note: "not that file"))
        #expect(Card.make(item: item("r", .review, task: "t-1")).replyAction("tests?") == .reviewSendBackNote(task: "t-1", note: "tests?"))
        #expect(Card.make(item: item("f", .finished)).replyAction("now docs") == .sendNextStep(session: "s-1", text: "now docs"))
        #expect(Card.make(item: item("f", .finished, session: nil)).replyAction("x") == nil)
        #expect(Card.make(item: item("s", .show)).replyAction("x") == nil)
    }

    @Test func grammarGainsReplyAndAskBar() {
        #expect(KeyGrammar.intent(for: KeyPress(chars: "r")) == .reply)
        #expect(KeyGrammar.intent(for: KeyPress(chars: "R")) == .reply)
        #expect(KeyGrammar.intent(for: KeyPress(chars: "l", command: true)) == .askBar)
        #expect(KeyGrammar.intent(for: KeyPress(chars: "l")) == .none)
        #expect(KeyGrammar.intent(for: KeyPress(chars: "r", command: true)) == .none)
    }

    @Test func panelFocusWalksIntoTheDoneZone() {
        var p = PanelModel(items: [item("ask:1", .ask), item("finished:1", .finished)])
        p.receiptCount = 2
        p.move(1); p.move(1)
        #expect(p.focused == nil && p.receiptFocus == 0 && p.others.count == 2 && p.position == "2 of 2")
        p.move(1); p.move(1)
        #expect(p.receiptFocus == 1, "clamped at the last receipt")
        p.move(-2)
        #expect(p.focused?.id == "finished:1" && p.receiptFocus == nil)
        p.receiptCount = 0
        p.move(5)
        #expect(p.focused?.id == "finished:1", "no receipts: the old clamp")
    }
}

struct AskVoiceTests {
    @Test func tapTogglesAndNeverRecords() {
        var m = AskVoiceMachine()
        #expect(m.handle(.keyDown(t0)) == [])
        #expect(m.handle(.tick(t0.addingTimeInterval(0.1))) == [])
        #expect(m.handle(.keyUp(t0.addingTimeInterval(0.2))) == [.tap])
        #expect(m.state == .idle)
    }

    @Test func holdRecordsThenTranscribesIntoTheField() {
        var m = AskVoiceMachine()
        _ = m.handle(.keyDown(t0))
        #expect(m.handle(.tick(t0.addingTimeInterval(0.249))) == [])
        #expect(m.handle(.tick(t0.addingTimeInterval(0.25))) == [.startRecording])
        #expect(m.isRecording)
        #expect(m.handle(.tick(t0.addingTimeInterval(3))) == [], "ticks while recording are quiet until the cap")
        #expect(m.handle(.keyUp(t0.addingTimeInterval(4))) == [.stopAndTranscribe])
        #expect(m.handle(.keyDown(t0.addingTimeInterval(4.1))) == [], "a press while transcribing is ignored")
        #expect(m.handle(.transcript("  ship it \n")) == [.setText("ship it")])
        #expect(m.state == .idle)
    }

    @Test func emptyTranscriptAndErrorsFail() {
        var m = AskVoiceMachine()
        _ = m.handle(.keyDown(t0)); _ = m.handle(.tick(t0.addingTimeInterval(1))); _ = m.handle(.keyUp(t0.addingTimeInterval(2)))
        #expect(m.handle(.transcript("  ")) == [.fail("Nothing recognized")])
        _ = m.handle(.keyDown(t0)); _ = m.handle(.tick(t0.addingTimeInterval(1)))
        #expect(m.handle(.failed("no microphone")) == [.fail("no microphone")])
        #expect(m.state == .idle)
        #expect(m.handle(.keyUp(t0)) == [], "a key-up with nothing pressed is nothing")
    }

    @Test func recordingStopsAtTheCap() {
        var m = AskVoiceMachine()
        _ = m.handle(.keyDown(t0)); _ = m.handle(.tick(t0.addingTimeInterval(1)))
        #expect(m.handle(.tick(t0.addingTimeInterval(1 + AskVoiceMachine.maxDuration))) == [.stopAndTranscribe])
        #expect(m.handle(.keyUp(t0.addingTimeInterval(70))) == [], "the release after an auto-stop does nothing")
    }

    @Test func wavHeaderIsCanonical() {
        let d = WAV.encode(pcm16: [0, 1, -1])
        #expect(d.count == 50)
        #expect(String(decoding: d[0..<4], as: UTF8.self) == "RIFF")
        #expect(String(decoding: d[8..<16], as: UTF8.self) == "WAVEfmt ")
        #expect(d[20] == 1 && d[22] == 1, "PCM, mono")
        #expect(d[24..<28].withUnsafeBytes { $0.load(as: UInt32.self).littleEndian } == 16_000)
        #expect(d[40..<44].withUnsafeBytes { $0.load(as: UInt32.self).littleEndian } == 6)
        #expect(Array(d[44...]) == [0, 0, 1, 0, 0xFF, 0xFF])
    }
}
