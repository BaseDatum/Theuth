import Foundation
import Testing
import TheuthKit
@testable import CompanionCore

// 189 COMPANION-STREAM: one focus owner (D1), the transcript catch-up (D2), the terminal stop reasons (D3).

struct FocusCoordinatorTests {
    @Test func theEngineGetsTheUnionOfEveryClaim() {
        var c = FocusCoordinator()
        #expect(c.claim(.panel, ["card-1"]) == ["card-1"])
        _ = c.finished(["card-1"])
        #expect(c.claim(.ask, ["desk"]) == ["card-1", "desk"])
        _ = c.finished(["card-1", "desk"])
        #expect(c.claim(.point, ["desk"]) == nil, "the desk twice is one session: nothing new to send")
        #expect(c.claim(.ask, []) == nil, "the voice key still holds the desk")
        #expect(c.claim(.point, []) == ["card-1"], "the last claim on the desk withdrawn")
    }

    @Test func sendsAreSerializedAndTheLatestUnionWins() {
        var c = FocusCoordinator()
        #expect(c.claim(.ask, ["desk"]) == ["desk"])
        #expect(c.claim(.panel, ["card-1"]) == nil, "one send in flight")
        #expect(c.claim(.panel, ["card-2"]) == nil)
        #expect(c.finished(["desk"]) == ["card-2", "desk"], "the union as it is now, never the stale card-1 one")
        #expect(c.finished(["card-2", "desk"]) == nil, "nothing moved meanwhile")
        #expect(c.claim(.panel, ["card-2"]) == nil, "an unchanged union is not resent")
    }

    @Test func aRefusedSendIsNotRetriedInALoop() {
        var c = FocusCoordinator()
        let u = c.claim(.ask, ["desk"])
        #expect(u == ["desk"])
        #expect(c.finished(u!) == nil, "an engine before 129 refuses every time: the caller reports it, nothing loops")
    }

    @Test func aReconnectOrPendingPromptsResendsTheUnion() {
        var c = FocusCoordinator()
        _ = c.claim(.ask, ["desk"])
        _ = c.finished(["desk"])
        #expect(c.resend() == ["desk"], "a new connection has no focus")
        #expect(c.resend() == nil, "in flight: the forced resend waits")
        #expect(c.finished(["desk"]) == ["desk"], "a resend asked for during a send goes out after it")
        #expect(c.finished(["desk"]) == nil)
        var fresh = FocusCoordinator()
        #expect(fresh.resend() == [], "no claims yet: the empty set (155 §13.5: nothing in view)")
    }

    @Test func closingThePanelWhileAnAskCollectsKeepsTheDesk() {
        var c = FocusCoordinator()
        _ = c.finished(c.claim(.ask, ["desk"])!)
        let open = c.claim(.panel, ["card-1"])
        #expect(open == ["card-1", "desk"], "opening the panel adds its card, the desk stays")
        _ = c.finished(open!)
        #expect(c.claim(.panel, []) == ["desk"], "the incident: closing the panel sent [] and every reply chunk was dropped")
    }
}

/// A `load_session` reply as the engine renders it (`crates/agent/src/lib.rs` `Loaded`, `HistoryEntry`).
private let page = """
{"history":[
 {"role":"user","text":"what is up","turn":10},
 {"role":"assistant","text":"old answer","turn":10},
 {"role":"user","text":"  status of w3? ","turn":14,"context":null},
 {"role":"reasoning","text":"checking the board","turn":14,"thinking_ms":400},
 {"role":"assistant","text":"Let me check.","turn":14},
 {"role":"tool","call_id":"c1","name":"shell","args":{"command":"ls"},"result":["ok",false]},
 {"role":"assistant","text":"w3 is **done**.","turn":14}
],"running":false,"model":"m","effort":null,"queued":[],"undone":0,"agent":null,"name":null,"parent":null,"skills":[]}
"""

private func loaded(_ json: String) throws -> Loaded { try TheuthJSON.decoder.decode(Loaded.self, from: Data(json.utf8)) }

struct TranscriptPickTests {
    @Test func theLastAssistantRowAfterTheAsksOwnUserRow() throws {
        let l = try loaded(page)
        #expect(l.history.count == 7 && l.running == false, "tool and reasoning rows decode too")
        #expect(TranscriptPick.reply(in: l.history, question: "status of w3?", turnSeq: nil) == .init(text: "w3 is **done**.", anchored: true),
                "matched by text (whitespace aside); the final row, not the one before the tool call")
        #expect(TranscriptPick.reply(in: l.history, question: "status of w3?", turnSeq: 10) == .init(text: "old answer", anchored: true),
                "the turn id wins over the text")
        #expect(TranscriptPick.reply(in: l.history, question: "status of w3?", turnSeq: 99).text == "w3 is **done**.",
                "a turn id outside the page falls back to the text")
        #expect(TranscriptPick.reply(in: l.history, question: "[voice note, 3 s] not here", turnSeq: nil) == .init(text: "w3 is **done**.", anchored: false),
                "our row outside the page: the newest assistant row")
    }

    @Test func noAssistantRowAfterTheAsk() {
        let h = [HistoryEntry(role: "assistant", text: "old answer", turn: 10), HistoryEntry(role: "user", text: "and now?", turn: 12)]
        #expect(TranscriptPick.reply(in: h, question: "and now?", turnSeq: nil) == .init(text: nil, anchored: true), "never the previous answer")
        #expect(TranscriptPick.reply(in: [], question: "and now?", turnSeq: nil) == .init(text: nil, anchored: false))
    }

    /// The live desk (smoke, 2026-11-11): peer messages and job results open turns of their own between ours.
    @Test func aLaterTurnsAnswerIsNotOurs() {
        let h = [
            HistoryEntry(role: "user", text: "ship it?", turn: 40),
            HistoryEntry(role: "user", text: "<knowledge>…</knowledge>"),
            HistoryEntry(role: "assistant", text: "shipped", turn: 40),
            HistoryEntry(role: "user", text: "<message from=\"session:01a1\">…</message>", turn: 43),
            HistoryEntry(role: "assistant", text: "I didn't reply to the worker.", turn: 43),
        ]
        #expect(TranscriptPick.reply(in: h, question: "ship it?", turnSeq: nil).text == "shipped")
        let steer = [HistoryEntry(role: "user", text: "and the tests", turn: nil), HistoryEntry(role: "assistant", text: "running them", turn: 40)]
        #expect(TranscriptPick.reply(in: steer, question: "and the tests", turnSeq: nil).text == "running them", "a steer row has no turn of its own")
    }
}

struct CatchUpTests {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    private func asked(_ q: String = "status of w3?") -> AskRouter {
        var r = AskRouter()
        r.sent(on: "desk", question: q, now: t0)
        return r
    }

    @Test func afterTheTurnTheTranscriptReplacesTheStream() throws {
        var r = asked()
        r.observe(.textDelta(session: "desk", text: "Let me check.w3 is done"), now: t0)
        r.observe(.turnFinished(session: "desk", stopReason: "end_turn"), now: t0)
        #expect(r.caughtUp(id: r.reply!.id, loaded: try loaded(page), trigger: .turnFinished, now: t0) == true)
        #expect(r.reply?.finished == true && r.reply?.failure == nil)
        #expect(r.reply?.text == "w3 is **done**.", "the transcript is authoritative")
    }

    @Test func deltasDroppedEntirelyStillEndInTheAnswer() throws {
        var r = asked()
        r.observe(.turnFinished(session: "desk", stopReason: "end_turn"), now: t0)
        #expect(r.reply?.finished == false, "the old guard ignored this finish: thinking forever")
        r.caughtUp(id: r.reply!.id, loaded: try loaded(page), trigger: .turnFinished, now: t0)
        #expect(r.reply?.text == "w3 is **done**." && r.reply?.finished == true)
    }

    @Test func runningKeepsCollecting() {
        var r = asked("and now?")
        r.observe(.turnFinished(session: "desk", stopReason: "end_turn"), now: t0) // the turn we steered into
        let h = [HistoryEntry(role: "user", text: "and now?", turn: 20), HistoryEntry(role: "assistant", text: "first round", turn: 20)]
        #expect(r.caughtUp(id: r.reply!.id, loaded: Loaded(history: h, running: true), trigger: .turnFinished, now: t0) == true)
        #expect(r.reply?.finished == false && r.reply?.turnEnded == false, "running: ours is still going")
        #expect(r.reply?.text == "first round", "an empty stream is filled from our own rows")
        let other = [HistoryEntry(role: "assistant", text: "someone else's answer")]
        var s = asked("and now?")
        #expect(s.caughtUp(id: s.reply!.id, loaded: Loaded(history: other, running: true), trigger: .lagged, now: t0) == false)
        #expect(s.reply?.text == "", "unanchored while running: never shows another answer")
    }

    @Test func noAssistantTextEndsWithoutAReplyNotThinking() {
        var r = asked("and now?")
        r.observe(.turnFinished(session: "desk", stopReason: "end_turn"), now: t0)
        let h = [HistoryEntry(role: "user", text: "and now?", turn: 12)]
        #expect(r.caughtUp(id: r.reply!.id, loaded: Loaded(history: h, running: false), trigger: .turnFinished, now: t0) == true)
        #expect(r.reply?.finished == true)
        #expect(r.reply?.failure == AskReply.noReply)
        #expect(r.reply?.receiptText == "theuth finished without a reply", "read as is, not 'couldn't reply — …'")
        var e = asked()
        e.observe(.turnFinished(session: "desk", stopReason: "end_turn"), now: t0)
        e.caughtUp(id: e.reply!.id, loaded: nil, trigger: .turnFinished, now: t0)
        #expect(e.reply?.failure == AskReply.noReply, "a failed read after the finish, nothing streamed: still ends")
    }

    @Test func withoutAFinishOnlyOurAnsweredRowOrTheWatchdogEndsIt() throws {
        let unanchored = Loaded(history: [HistoryEntry(role: "assistant", text: "older")], running: false)
        var r = asked()
        #expect(r.caughtUp(id: r.reply!.id, loaded: unanchored, trigger: .reconnect, now: t0) == false,
                "a reconnect right after the send: the previous answer is not ours")
        #expect(r.reply?.finished == false)
        #expect(r.caughtUp(id: r.reply!.id, loaded: try loaded(page), trigger: .lagged, now: t0) == true, "our row answered: the lost finish is not needed")
        #expect(r.reply?.text == "w3 is **done**." && r.reply?.finished == true)
        var w = asked()
        #expect(w.caughtUp(id: w.reply!.id, loaded: unanchored, trigger: .watchdog, now: t0) == true)
        #expect(w.reply?.finished == true && w.reply?.text == "older", "the watchdog's quiet desk: the newest answer stands in")
    }

    @Test func aLaterTurnStartingUndoesAnEarlierFinish() {
        var r = asked("and now?")
        r.observe(.turnFinished(session: "desk", stopReason: "end_turn"), now: t0)
        r.observe(.turnStarted(session: "desk", turnSeq: 30, text: "and now?"), now: t0)
        #expect(r.reply?.turnSeq == 30 && r.reply?.turnEnded == false)
        let h = [HistoryEntry(role: "user", text: "and now?", turn: 30)]
        #expect(r.caughtUp(id: r.reply!.id, loaded: Loaded(history: h, running: false), trigger: .turnFinished, now: t0) == false,
                "a read issued at the earlier finish cannot end the turn that started after it")
        var s = asked("and now?")
        s.observe(.turnStarted(session: "desk", turnSeq: 31, text: "someone else"), now: t0)
        #expect(s.reply?.turnSeq == nil, "another text's turn is not ours")
    }

    @Test func aCatchUpForAnEarlierSendIsDropped() throws {
        var r = asked()
        let old = r.reply!.id
        r.sent(on: "desk", question: "next", now: t0)
        r.observe(.turnFinished(session: "desk", stopReason: "end_turn"), now: t0)
        #expect(r.caughtUp(id: old, loaded: try loaded(page), trigger: .watchdog, now: t0) == false)
        #expect(r.reply?.finished == false)
    }
}

struct StopReasonTests {
    @Test func terminalStopReasonsEndAsFailures() {
        let cases = ["error": "the turn failed", "cancelled": "the turn was cancelled", "suspended": "the turn was suspended",
                     "timed_out": "the turn timed out", "interrupted": "the turn was interrupted"]
        for (reason, why) in cases {
            var r = AskRouter()
            r.sent(on: "desk")
            #expect(r.observe(.turnFinished(session: "desk", stopReason: reason)) == true)
            #expect(r.reply?.finished == true && r.reply?.failure == why, "\(reason)")
            #expect(r.reply?.receiptText == "couldn't reply — \(why)")
            var c = CommandCard(request: "x")
            c.observe(.turnFinished(stopReason: reason))
            #expect(c.frozen == "✗ couldn't finish — \(why)", "the command card reads the same set")
        }
    }

    @Test func everyOtherStopReasonIsANormalFinish() {
        for reason in ["end_turn", "max_tokens", "stop", "tool_use", ""] {
            #expect(AskReply.terminalFailure(reason) == nil)
            var r = AskRouter()
            r.sent(on: "desk")
            r.observe(.turnFinished(session: "desk", stopReason: reason))
            #expect(r.reply?.finished == false && r.reply?.turnEnded == true, "\(reason): the transcript decides")
        }
    }
}

struct WatchdogTests {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    @Test func firesAfterFifteenQuietSecondsAndRearms() {
        var r = AskRouter()
        r.sent(on: "desk", now: t0)
        #expect(r.reply?.watchdogDeadline == t0.addingTimeInterval(15))
        #expect(r.watchdogFired(now: t0.addingTimeInterval(14.9)) == false)
        r.observe(.activity(session: "desk"), now: t0.addingTimeInterval(10))
        #expect(r.watchdogFired(now: t0.addingTimeInterval(20)) == false, "any desk event re-arms it")
        r.observe(.activity(session: "other"), now: t0.addingTimeInterval(24))
        #expect(r.watchdogFired(now: t0.addingTimeInterval(25)) == true, "another session's events do not")
        #expect(r.reply?.watchdogDeadline == t0.addingTimeInterval(40), "re-armed from the fire, while still running")
        #expect(r.watchdogFired(now: t0.addingTimeInterval(25.5)) == false)
        r.caughtUp(id: r.reply!.id, loaded: Loaded(history: [], running: true), trigger: .watchdog, now: t0.addingTimeInterval(26))
        #expect(r.reply?.watchdogDeadline == t0.addingTimeInterval(41), "a running catch-up re-arms it too")
        r.observe(.error(session: "desk", message: "boom"), now: t0.addingTimeInterval(27))
        #expect(r.reply?.watchdogDeadline == nil, "finished: off")
        #expect(r.watchdogFired(now: t0.addingTimeInterval(99)) == false)
        var idle = AskRouter()
        #expect(idle.watchdogFired(now: t0) == false, "nothing sent: off")
    }
}
