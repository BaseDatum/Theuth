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
        #expect(r.observe(.textDelta(session: "other", text: "noise")) == false)
        #expect(r.observe(.textDelta(session: "desk", text: "- handed to w3 — it owns crates/rpc\n")) == true)
        #expect(r.observe(.textDelta(session: "desk", text: "told w2 to wait\nthird line")) == true)
        #expect(r.reply?.firstTwoLines == "handed to w3 — it owns crates/rpc — told w2 to wait")
        #expect(r.observe(.turnFinished(session: "desk", stopReason: "end_turn")) == true)
        #expect(r.reply?.finished == false, "189 D2: a normal finish waits for the transcript")
        #expect(r.caughtUp(id: r.reply!.id, loaded: nil, trigger: .turnFinished, now: Date()) == true, "a failed read ends on what streamed")
        #expect(r.reply?.failure == nil)
        #expect(r.reply?.receiptText == "handed to w3 — it owns crates/rpc — told w2 to wait", "a normal reply reads as before")
        #expect(r.reply?.finished == true)
        #expect(r.observe(.textDelta(session: "desk", text: "late")) == false, "finished replies stay put")
        #expect(r.reply?.text.contains("late") == false)
    }

    /// A failed desk turn must end the receipt instead of leaving "sent — waiting for the desk" forever (134 W6).
    @Test func aFailedOrCancelledDeskTurnEndsTheReplyWithTheReason() {
        var r = AskRouter()
        r.sent(on: "desk")
        let long = "model: Resolver error for \"anthropic/claude\": no API key. Cause: ApiKeyEnvNotFound { env_name: \"ANTHROPIC_API_KEY\" }\nsecond line"
        #expect(r.observe(.error(session: "other", message: long)) == false, "another session's error is not ours")
        #expect(r.observe(.error(session: "desk", message: long)) == true)
        #expect(r.reply?.finished == true)
        #expect(r.reply?.failure == "model: Resolver error for \"anthropic/claude\": no API key. Cause: ApiKeyEnvNotFo…", "first line, clipped to 80")
        #expect(r.reply?.receiptText.hasPrefix("couldn't reply — model: Resolver error") == true)
        #expect(r.observe(.turnFinished(session: "desk", stopReason: "error")) == false, "the trailing TurnFinished changes nothing")

        r.sent(on: "desk")
        #expect(r.observe(.turnFinished(session: "desk", stopReason: "error")) == true, "stop_reason error with nothing streamed still ends it")
        #expect(r.reply?.receiptText == "couldn't reply — the turn failed")

        r.sent(on: "desk")
        r.observe(.textDelta(session: "desk", text: "half an answ"))
        #expect(r.observe(.turnFinished(session: "desk", stopReason: "cancelled")) == true)
        #expect(r.reply?.receiptText == "couldn't reply — the turn was cancelled")
        #expect(r.reply?.finished == true)

        r.sent(on: "desk")
        #expect(r.observe(.error(session: "desk", message: "  \n")) == true)
        #expect(r.reply?.receiptText == "couldn't reply — the turn failed", "an empty message still reads")
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

    @Test func holdStreamsThenTheDictatedTextLandsInTheField() {
        var m = AskVoiceMachine()
        _ = m.handle(.keyDown(t0))
        #expect(m.handle(.tick(t0.addingTimeInterval(0.249))) == [])
        #expect(m.handle(.tick(t0.addingTimeInterval(0.25))) == [.startRecording])
        #expect(m.isRecording)
        #expect(m.handle(.tick(t0.addingTimeInterval(3))) == [], "ticks while recording are quiet until the cap")
        #expect(m.handle(.keyUp(t0.addingTimeInterval(4))) == [.stopDictation])
        #expect(m.handle(.keyDown(t0.addingTimeInterval(4.1))) == [], "a press while stopping is ignored")
        #expect(m.handle(.dictated("  ship it \n")) == [.setText("ship it")])
        #expect(m.state == .idle)
    }

    @Test func emptyTranscriptAndErrorsFail() {
        var m = AskVoiceMachine()
        _ = m.handle(.keyDown(t0)); _ = m.handle(.tick(t0.addingTimeInterval(1))); _ = m.handle(.keyUp(t0.addingTimeInterval(2)))
        #expect(m.handle(.dictated("  ")) == [.fail("Nothing recognized")])
        _ = m.handle(.keyDown(t0)); _ = m.handle(.tick(t0.addingTimeInterval(1)))
        #expect(m.handle(.failed("no microphone")) == [.fail("no microphone")])
        #expect(m.state == .idle)
        #expect(m.handle(.keyUp(t0)) == [], "a key-up with nothing pressed is nothing")
    }

    @Test func recordingStopsAtTheCap() {
        var m = AskVoiceMachine()
        _ = m.handle(.keyDown(t0)); _ = m.handle(.tick(t0.addingTimeInterval(1)))
        #expect(m.handle(.tick(t0.addingTimeInterval(1 + AskVoiceMachine.maxDuration))) == [.stopDictation])
        #expect(m.handle(.keyUp(t0.addingTimeInterval(70))) == [], "the release after an auto-stop does nothing")
    }
}

/// 149g F2: the Ask bar's dictation lifecycle as a pure driver — `AskVoiceMachine` + `AskDictation` + the hold generation,
/// `AskBarController.apply/beginDictation/endDictation` without AppKit — over a fake clock and a fake RPC (`rpc` is the calls
/// in order; a `dictation_start` reply is landed by hand, so it can land after its hold ended).
struct AskDictationHoldTests {
    struct Driver {
        var voice = AskVoiceMachine()
        var dictation = AskDictation()
        var hold = 0
        var stopWanted = false
        var rpc: [String] = []
        var status: String?

        mutating func press(_ t: TimeInterval) { apply(voice.handle(.keyDown(t0.addingTimeInterval(t)))) }
        mutating func tick(_ t: TimeInterval) { apply(voice.handle(.tick(t0.addingTimeInterval(t)))) }
        mutating func release(_ t: TimeInterval) { apply(voice.handle(.keyUp(t0.addingTimeInterval(t)))) }
        /// ⎋ in the bar and the panel closing are one path (`AskBarController.close`).
        mutating func close() { if voice.isBusy { apply(voice.handle(.failed("cancelled"))) } }

        mutating func apply(_ fx: [AskVoiceMachine.Effect]) {
            for f in fx {
                switch f {
                case .startRecording:
                    _ = dictation.handle(.start); stopWanted = false
                    hold += 1; rpc.append("start#\(hold)")
                case .stopDictation:
                    end()
                case .fail(let why):
                    if let id = dictation.id { rpc.append("stop \(id)") }
                    dictation = AskDictation(); stopWanted = false
                    status = why
                case .tap, .setText, .openMic, .discardMic, .sendNow: break
                }
            }
        }

        /// The engine's `dictation_start` reply for hold `g` lands (`beginDictation`'s success path).
        mutating func startLanded(_ g: Int, _ id: String) {
            guard dictation.awaitsStart(from: g, current: hold) else { return rpc.append("stop \(id) (late)") }
            _ = dictation.handle(.started(id: id))
            if stopWanted { stopWanted = false; end() }
        }

        /// The engine refuses hold `g`'s start (`beginDictation`'s failure path).
        mutating func startFailed(_ g: Int, _ why: String) {
            guard dictation.awaitsStart(from: g, current: hold) else { return }
            _ = dictation.handle(.startFailed(why))
            apply(voice.handle(.failed(dictation.error ?? why)))
        }

        mutating func end() {
            guard let id = dictation.id else { stopWanted = dictation.phase == .starting; return }
            _ = dictation.handle(.stop); rpc.append("stop \(id)")
        }
    }

    /// Past the hold threshold: the start is in flight.
    func recording() -> Driver {
        var d = Driver()
        d.press(0); d.tick(0.3)
        #expect(d.voice.isRecording && d.dictation.phase == .starting && d.rpc == ["start#1"])
        return d
    }

    @Test func escBeforeTheStartRepliesStopsTheLateIdAtOnce() {
        var d = recording()
        d.close()
        #expect(d.voice.state == .idle && !d.dictation.isActive && d.rpc == ["start#1"], "nothing to stop yet; the session is reset")
        d.startLanded(1, "a")
        #expect(d.rpc == ["start#1", "stop a (late)"])
        #expect(!d.dictation.isActive, "never adopted")
        let took = d.dictation.handle(.started(id: "a"))
        #expect(!took && !d.dictation.isActive, "a .started reaching a reset session is ignored")
    }

    @Test func releaseBeforeTheStartRepliesAdoptsThenStops() {
        var d = recording()
        d.release(1)
        #expect(d.voice.state == .stopping && d.stopWanted && d.rpc == ["start#1"])
        d.startLanded(1, "a")
        #expect(d.rpc == ["start#1", "stop a"] && d.dictation.phase == .stopping(id: "a"))
    }

    @Test func aLateStartNeverLandsInTheNextHold() {
        var d = recording()
        d.close()
        d.press(2); d.tick(2.3)
        #expect(d.rpc == ["start#1", "start#2"] && d.dictation.phase == .starting)
        d.startLanded(1, "a")
        #expect(d.rpc.last == "stop a (late)" && d.dictation.phase == .starting, "the second hold still waits for its own start")
        d.startLanded(2, "b")
        #expect(d.dictation.phase == .live(id: "b") && d.voice.isRecording)
        d.release(4)
        #expect(d.rpc.last == "stop b")
    }

    @Test func aStaleRefusalDoesNotEndTheNextHold() {
        var d = recording()
        d.close()
        d.press(2); d.tick(2.3)
        d.startFailed(1, "already dictating")
        #expect(d.voice.isRecording && d.dictation.phase == .starting && d.status == "cancelled", "hold 1's refusal is not hold 2's")
        d.startFailed(2, "already dictating")
        #expect(d.voice.state == .idle && !d.dictation.isActive && d.status == "dictating elsewhere")
        #expect(d.rpc == ["start#1", "start#2"], "a refused start has no id to stop")
    }
}
