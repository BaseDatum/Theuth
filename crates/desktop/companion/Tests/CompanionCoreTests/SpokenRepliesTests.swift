import Testing
import Foundation
@testable import CompanionCore

/// Records the engine calls; `gate` holds the `speak` reply until the test releases it.
private actor FakeSpeechClient: SpeechClient {
    var speaks: [(session: String, text: String)] = []
    var stops: [String?] = []
    private var gate: CheckedContinuation<Void, Never>?
    private var gated = false
    private var n = 0

    func hold() { gated = true }
    func release() { gated = false; gate?.resume(); gate = nil }

    func speak(session: String, text: String) async throws -> String {
        speaks.append((session, text))
        if gated { await withCheckedContinuation { gate = $0 } }
        n += 1
        return "sp-\(n)"
    }

    func stopSpeaking(id: String?) async throws { stops.append(id) }
}

@MainActor
private final class FakeSink: SpeechSink {
    var expected: [String] = []
    /// Mirrors `SpeechPlayer.expecting`: set by `expect`, cleared by `stop`.
    var expecting: String?
    var stops = 0
    func expect(session: String) { expected.append(session); expecting = session }
    func stop() { stops += 1; expecting = nil }
}

private func reply(_ text: String, finished: Bool, failure: String? = nil) -> AskReply {
    var r = AskReply(session: "desk")
    r.text = text; r.finished = finished; r.failure = failure
    return r
}

struct SpokenRepliesSettingsTests {
    @Test func bothOffUntilSet() throws {
        let suite = "io.theuth.mac.companion.tests.\(UUID().uuidString)"
        let d = try #require(UserDefaults(suiteName: suite))
        defer { d.removePersistentDomain(forName: suite) }
        #expect(SpokenReplies.voiceKey(d) == false, "S1: the old always-on default is gone")
        #expect(SpokenReplies.ask(d) == false)
        d.set(true, forKey: "voiceKeySpeak")
        #expect(SpokenReplies.voiceKey(d) == true && SpokenReplies.ask(d) == false, "independent")
        d.set(true, forKey: "askSpeak")
        d.set(false, forKey: "voiceKeySpeak")
        #expect(SpokenReplies.voiceKey(d) == false && SpokenReplies.ask(d) == true, "an explicit value is kept")
    }

    @Test func askSpeakDecision() {
        let final = reply("All green.", finished: true)
        #expect(SpokenReplies.shouldSpeak(enabled: true, reply: final))
        #expect(!SpokenReplies.shouldSpeak(enabled: false, reply: final), "off: nothing is spoken")
        #expect(!SpokenReplies.shouldSpeak(enabled: true, reply: reply("All gr", finished: false)), "partials wait")
        #expect(!SpokenReplies.shouldSpeak(enabled: true, reply: reply("", finished: true)), "empty says nothing")
        #expect(!SpokenReplies.shouldSpeak(enabled: true, reply: reply("partial", finished: true, failure: "the turn failed")), "a failure says nothing")
    }
}

@MainActor
struct SpeakerTests {
    @Test func speaksThenStopsTheEngineUtteranceById() async {
        let client = FakeSpeechClient(), sink = FakeSink()
        let s = Speaker(sink: sink); s.client = client
        await s.speak("All green.", on: "desk", from: .askBar).value
        #expect(s.surface == .askBar && s.id == "sp-1")
        #expect(sink.expected == ["desk"])
        let spoke = await client.speaks
        #expect(spoke.count == 1 && spoke[0].text == "All green." && spoke[0].session == "desk")
        s.stop()
        #expect(s.surface == nil && s.id == nil && sink.stops >= 1)
        await Task.yield(); await Task.yield()
        #expect(await client.stops == ["sp-1"], "Esc / a new ask stops the current id, nothing else")
        s.stop()
        await Task.yield()
        #expect(await client.stops == ["sp-1"], "nothing speaking: no second stop")
    }

    @Test func aNewSpeakSilencesTheOtherSurfaceFirst() async {
        let client = FakeSpeechClient(), sink = FakeSink()
        let s = Speaker(sink: sink); s.client = client
        await s.speak("ask reply", on: "desk", from: .askBar).value
        await s.speak("voice reply", on: "desk", from: .voiceKey).value
        await Task.yield(); await Task.yield()
        #expect(await client.stops == ["sp-1"], "the Ask bar's utterance is stopped before the voice key's starts")
        #expect(s.surface == .voiceKey && s.id == "sp-2")
    }

    @Test func anIdLandingAfterStopIsStoppedAtOnce() async {
        let client = FakeSpeechClient(), sink = FakeSink()
        let s = Speaker(sink: sink); s.client = client
        await client.hold()
        let t = s.speak("late", on: "desk", from: .askBar)
        await Task.yield()
        s.stop() // Esc before the engine answered: no id yet
        #expect(s.surface == nil)
        #expect(await client.stops.isEmpty)
        await client.release()
        await t.value
        await Task.yield(); await Task.yield()
        #expect(s.id == nil && s.surface == nil, "the late id is not adopted")
        #expect(await client.stops == ["sp-1"], "…but the engine is told to stop it")
    }

    @Test func playbackEndingClearsTheSurface() async {
        let client = FakeSpeechClient(), sink = FakeSink()
        let s = Speaker(sink: sink); s.client = client
        await s.speak("done soon", on: "desk", from: .voiceKey).value
        s.ended("other-id")
        #expect(s.surface == .voiceKey, "a foreign id is not ours")
        s.ended("sp-1")
        #expect(s.surface == nil && s.id == nil)
        await Task.yield()
        #expect(await client.stops.isEmpty, "a natural end sends no stop_speaking")
    }

    @Test func speakFailureGoesQuiet() async {
        struct Refusing: SpeechClient {
            func speak(session: String, text: String) async throws -> String { throw URLError(.cannotConnectToHost) }
            func stopSpeaking(id: String?) async throws {}
        }
        let sink = FakeSink()
        let s = Speaker(sink: sink); s.client = Refusing()
        await s.speak("x", on: "desk", from: .askBar).value
        #expect(s.surface == nil && s.id == nil && sink.expecting == nil, "no utterance will come: the sink stops waiting for one")
    }
}

/// 149s K1: the two surface keys open different things, whatever the inbox holds.
struct SurfaceKeyTests {
    private let t0 = Date(timeIntervalSince1970: 1_791_448_800)

    @Test func tapOnThePanelKeyTogglesThePanelEvenWhenNothingNeedsYou() {
        var m = AskVoiceMachine()
        #expect(m.handle(.keyDown(t0)).isEmpty)
        let fx = m.handle(.keyUp(t0.addingTimeInterval(0.1)))
        #expect(fx == [.tap])
        #expect(SurfaceKey.panel.action(held: false) == .togglePanel, "never the Ask bar")
    }

    @Test func holdOnThePanelKeyStillTalks() {
        var m = AskVoiceMachine()
        _ = m.handle(.keyDown(t0))
        #expect(m.handle(.tick(t0.addingTimeInterval(AskVoiceMachine.holdThreshold))) == [.startRecording])
        #expect(SurfaceKey.panel.action(held: true) == .talk)
    }

    @Test func theAskBarKeyOpensTheAskBarOnly() {
        #expect(SurfaceKey.askBar.action(held: false) == .toggleAskBar)
        #expect(SurfaceKey.askBar.action(held: true) == .toggleAskBar)
    }
}
