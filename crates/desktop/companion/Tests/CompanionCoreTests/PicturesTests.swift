import Foundation
import Testing
@testable import TheuthKit
@testable import CompanionCore

// 192 COMPANION-PICTURES + THINKING: the segmenter (P1), the cache and the outcome mapping (P2), the thinking state (P3).

struct AnswerSegmentsTests {
    @Test func aClosedPictureFenceIsAPictureAnOpenOneIsDrawing() {
        let open = "Here:\n\n```mermaid\ngraph TD\n  A-->B"
        #expect(AnswerSegments.split(open, finished: false) == [.markdown("Here:\n"), .drawing(lang: "mermaid", title: nil)],
                "streaming: a placeholder, never the source")
        #expect(AnswerSegments.split(open + "\n```\nDone.", finished: false)
                == [.markdown("Here:\n"), .picture(PictureFence(lang: "mermaid", source: "graph TD\n  A-->B")), .markdown("Done.")])
        #expect(AnswerSegments.split(open, finished: true) == [.markdown(open)], "finished but never closed: a code block")
    }

    @Test func jsonrenderIsAnAliasAndTheRootTitleShowsWhileDrawing() {
        let s = AnswerSegments.split("```jsonrender\n{\"root\": {\"type\": \"Card\", \"title\": \"Spend this week\"", finished: false)
        #expect(s == [.drawing(lang: "json-render", title: "Spend this week")])
        let closed = AnswerSegments.split("```JSON-Render\n{}\n```", finished: false)
        #expect(closed == [.picture(PictureFence(lang: "json-render", source: "{}"))], "case-insensitive, the canonical lang")
    }

    @Test func twoFencesAndAPictureInsideText() {
        let text = "A\n```openui\n<x/>\n```\nB\n~~~~mermaid\ngraph LR\n~~~~\nC"
        #expect(AnswerSegments.split(text, finished: true) == [
            .markdown("A"), .picture(PictureFence(lang: "openui", source: "<x/>")), .markdown("B"),
            .picture(PictureFence(lang: "mermaid", source: "graph LR")), .markdown("C"),
        ])
    }

    @Test func otherFencesStayMarkdownAndHideWhatTheyContain() {
        let text = "```swift\nlet x = 1\n```\n````markdown\n```mermaid\ngraph TD\n```\n````"
        #expect(AnswerSegments.split(text, finished: true) == [.markdown(text)], "a mermaid fence quoted inside a markdown fence is text")
        #expect(AnswerSegments.split("    ```mermaid\n    x\n    ```", finished: true).count == 1, "four spaces: indented code, not a fence")
        #expect(AnswerSegments.split("```mermaid\nx\n``", finished: false) == [.drawing(lang: "mermaid", title: nil)], "a shorter run does not close")
        #expect(AnswerSegments.split("\n\n", finished: true) == [])
    }
}

struct FenceCacheTests {
    private func key(_ src: String, width: Int = 408, dark: Bool = true, scale: Double = 2) -> FenceKey {
        FenceKey(PictureFence(lang: "mermaid", source: src), width: width, dark: dark, scale: scale)
    }

    @Test func theKeyIsLangSourceWidthAppearanceScale() {
        #expect(key("a") == key("a"))
        #expect(key("a") != key("b"))
        #expect(key("a") != key("a", width: 300))
        #expect(key("a") != key("a", dark: false))
        #expect(key("a") != key("a", scale: 1))
        #expect(key("a").appearance == "dark" && key("a", dark: false).appearance == "light")
    }

    @Test func aDrawnKeyIsNeverRequestedAgainAndTwoRunAtOnce() {
        var c = FenceCache()
        #expect(c.request(key("a")) == .start)
        #expect(c.request(key("a")) == .pending, "asked again while drawing: no second request")
        #expect(c.request(key("b")) == .start)
        #expect(c.request(key("c")) == .pending, "a third waits")
        #expect(c.inFlight.count == 2 && c.queue == [key("c")])
        let png = FencePictureState.image(png: Data([1]), width: 10, height: 5)
        #expect(c.finished(key("a"), png) == key("c"), "a slot frees: the queued key starts")
        #expect(c.request(key("a")) == .hit(png))
        #expect(c.finished(key("b"), .fallback(reason: "failed")) == nil)
        #expect(c.request(key("b")) == .hit(.fallback(reason: "failed")), "a failure is cached too")
    }
}

struct FenceOutcomeTests {
    private let png = Data("png".utf8).base64EncodedString()

    @Test func doneWithAPictureDrawsEverythingElseFallsBack() {
        #expect(FencePictureState.from(.success(FenceSnapshotReply(outcome: "done", png: png, width: 408, height: 200)))
                == .image(png: Data("png".utf8), width: 408, height: 200))
        for outcome in ["no_renderer", "failed", "timed_out", "cancelled"] {
            guard case .fallback(let why) = FencePictureState.from(.success(FenceSnapshotReply(outcome: outcome, error: "x"))) else {
                Issue.record("\(outcome) drew"); continue
            }
            #expect(why == "\(outcome): x")
        }
        #expect(FencePictureState.from(.success(FenceSnapshotReply(outcome: "done", png: nil))) == .fallback(reason: "done without a picture"))
        #expect(FencePictureState.from(.success(FenceSnapshotReply(outcome: "done", png: "%%", width: 1, height: 1)))
                == .fallback(reason: "done without a picture"), "undecodable base64")
    }

    @Test func anEngineWithoutTheMethodFallsBack() {
        let missing = RpcClient.RpcError(code: -32601, message: "Method not found")
        #expect(FencePictureState.from(.failure(missing)) == .fallback(reason: "no fence_snapshot on this engine"))
        if case .fallback = FencePictureState.from(.failure(RpcClient.RpcError.notConnected)) {} else { Issue.record("not connected drew") }
    }

    @Test func theWireDecodes() throws {
        let r = try TheuthJSON.decoder.decode(FenceSnapshotReply.self, from: Data(#"{"outcome":"timed_out","png":null,"width":null,"height":null,"error":"20 s"}"#.utf8))
        #expect(r == FenceSnapshotReply(outcome: "timed_out", error: "20 s"))
    }
}

struct ThinkingTests {
    private let t0 = Date(timeIntervalSince1970: 1_800_000_000)

    @Test func theAnswersFirstTextCollapsesItWithItsDuration() {
        var r = AskRouter()
        r.sent(on: "desk", now: t0)
        r.observe(.reasoningDelta(session: "desk", text: "Let me check "), now: t0.addingTimeInterval(1))
        r.observe(.reasoningDelta(session: "other", text: "not ours"), now: t0.addingTimeInterval(2))
        r.observe(.reasoningDelta(session: "desk", text: "the board."), now: t0.addingTimeInterval(3))
        #expect(r.reply?.thinking.live == true && r.reply?.thinking.text == "Let me check the board.")
        #expect(r.reply?.thinking.summary == nil)
        r.observe(.textDelta(session: "desk", text: "w3 is done."), now: t0.addingTimeInterval(5.4))
        #expect(r.reply?.thinking.live == false)
        #expect(r.reply?.thinking.summary == "Thought for 4 s", "first reasoning → first text, by the clock")
        r.observe(.reasoningDelta(session: "desk", text: " Again."), now: t0.addingTimeInterval(6))
        #expect(r.reply?.thinking.live == false && r.reply?.thinking.text.hasSuffix("Again.") == true, "later thinking: collapsed, kept")
    }

    @Test func theEnginesOwnMeasureWins() {
        var t = Thinking()
        t.add("x", now: t0)
        t.done(ms: 2_600)
        t.end(now: t0.addingTimeInterval(9))
        #expect(t.summary == "Thought for 3 s")
    }

    @Test func noReasoningLeavesTheSpinner() {
        var r = AskRouter()
        r.sent(on: "desk", now: t0)
        r.observe(.textDelta(session: "desk", text: "hi"), now: t0)
        r.observe(.turnFinished(session: "desk", stopReason: "end_turn"), now: t0)
        #expect(r.reply?.thinking == Thinking(), "nothing to show, nothing to collapse")
    }

    @Test func aFinishWithoutTextCollapsesToo() {
        var r = AskRouter()
        r.sent(on: "desk", now: t0)
        r.observe(.reasoningDelta(session: "desk", text: "hmm"), now: t0)
        r.observe(.turnFinished(session: "desk", stopReason: "end_turn"), now: t0.addingTimeInterval(2))
        #expect(r.reply?.thinking.summary == "Thought for 2 s")
        var e = AskRouter()
        e.sent(on: "desk", now: t0)
        e.observe(.reasoningDelta(session: "desk", text: "hmm"), now: t0)
        e.caughtUp(id: e.reply!.id, loaded: Loaded(history: [], running: false), trigger: .watchdog, now: t0.addingTimeInterval(1))
        #expect(e.reply?.finished == true && e.reply?.thinking.live == false)
    }

    @Test func theCardCarriesItPerTurn() {
        var s = AnswerCardState()
        s.begin(question: "q", kind: .voiceNote)
        var t = Thinking()
        t.add("first", now: t0)
        s.thinking = t
        s.followUp(question: "q2")
        #expect(s.thinking == Thinking() && s.turns[0].thinking.text == "first", "a follow-up thinks anew")
    }
}
