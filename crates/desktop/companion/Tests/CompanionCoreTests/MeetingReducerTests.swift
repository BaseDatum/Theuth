import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W3: one test per `MeetingEvent` variant mirroring `UI/store/meeting.ts:156-220`, plus the brief-stack rule (`:181-190`).
struct MeetingReducerTests {
    let t0 = Date(timeIntervalSince1970: 1_000_000)
    let proc = MeetingProcess(pid: 1, name: "Zoom", bundleId: nil, active: true)

    func live(_ state: MeetingState = .recording, at: Date? = nil) -> MeetingSlice {
        var s = MeetingSlice.empty
        s.meeting = "m1"; s.session = "s1"; s.state = state; s.clockAt = at ?? t0
        return s
    }

    func brief(_ id: String, _ source: BriefSource = .auto, topic: String? = nil) -> Brief {
        Brief(id: id, topic: topic, title: id, bullets: ["a", "b", "c", "d"], source: source)
    }

    @Test func startedResetsToARecordingSlice() {
        var old = live(.paused)
        old.topics = [Topic(id: "t", label: "old", atMs: 1)]
        let s = MeetingReducer.reduce(old, .started(meeting: "m2", session: "s2", process: proc, at: "now"), now: t0)
        #expect(s.meeting == "m2" && s.session == "s2" && s.state == .recording)
        #expect(s.topics.isEmpty && s.clockAt == t0 && s.elapsedMs == 0)
    }

    @Test func stateUpdatesAndEndsTheCuesWhenNotLive() {
        var s = live()
        s.listening = true; s.approvalWaiting = true; s.progress = .init(stage: .aligning, pct: 50)
        let paused = MeetingReducer.reduce(s, .state(meeting: "m1", state: .paused, degraded: [.modelDown]), now: t0 + 10)
        #expect(paused.state == .paused && paused.degraded == [.modelDown])
        #expect(paused.elapsedMs == 10_000 && paused.clockAt == t0 + 10)
        #expect(paused.listening && paused.approvalWaiting) // still live
        #expect(paused.progress == nil) // not a Post state
        let done = MeetingReducer.reduce(paused, .state(meeting: "m1", state: .done, degraded: []), now: t0 + 20)
        #expect(!done.listening && !done.approvalWaiting && done.elapsedMs == 10_000) // paused: no clock
    }

    @Test func stateForAnotherMeetingIsIgnored() {
        let s = live()
        #expect(MeetingReducer.reduce(s, .state(meeting: "other", state: .done, degraded: []), now: t0) == s)
        // An empty slice adopts the first State it sees.
        let fresh = MeetingReducer.reduce(.empty, .state(meeting: "m9", state: .recording, degraded: []), now: t0)
        #expect(fresh.meeting == "m9" && fresh.state == .recording)
    }

    @Test func levelKeepsTheMeters() {
        let l = Level(meeting: "m1", micRms: 0.2, systemRms: 0.4)
        #expect(MeetingReducer.reduce(live(), .level(l), now: t0).level == l)
    }

    @Test func utterancePartialReplacesTheOpenLineOfItsTrack() {
        let p1 = Utterance(track: .system, t0Ms: 100, t1Ms: 200, text: "hel")
        let p2 = Utterance(track: .system, t0Ms: 100, t1Ms: 300, text: "hello")
        let mic = Utterance(track: .mic, t0Ms: 50, t1Ms: 60, text: "me")
        var s = MeetingReducer.reduce(live(), .utterance(meeting: "m1", utterance: p1, final: false), now: t0)
        s = MeetingReducer.reduce(s, .utterance(meeting: "m1", utterance: mic, final: false), now: t0)
        s = MeetingReducer.reduce(s, .utterance(meeting: "m1", utterance: p2, final: false), now: t0)
        #expect(s.lines.map(\.utterance.text) == ["me", "hello"] && s.lines.allSatisfy(\.open))
        let fin = MeetingReducer.reduce(s, .utterance(meeting: "m1", utterance: p2, final: true), now: t0)
        #expect(fin.lines.map(\.utterance.text) == ["me", "hello"] && fin.lines[1].open == false)
        #expect(MeetingReducer.reduce(s, .utterance(meeting: "zz", utterance: p2, final: true), now: t0) == s)
    }

    @Test func utteranceFinalWithSpeakerUpdatesInPlace() {
        let u = Utterance(track: .system, t0Ms: 100, t1Ms: 300, text: "hello")
        var s = MeetingReducer.reduce(live(), .utterance(meeting: "m1", utterance: u, final: true), now: t0)
        s = MeetingReducer.reduce(s, .utterance(meeting: "m1", utterance: Utterance(track: .system, t0Ms: 400, t1Ms: 500, text: "next"), final: true), now: t0)
        var labelled = u; labelled.speaker = "s2"
        let r = MeetingReducer.reduce(s, .utterance(meeting: "m1", utterance: labelled, final: true), now: t0)
        #expect(r.lines.count == 2 && r.lines[0].utterance.speaker == "s2" && r.lines[1].utterance.text == "next")
        // The cap: 200 kept, oldest dropped.
        var many = live()
        for i in 0..<(MeetingSlice.linesKept + 5) {
            many = MeetingReducer.reduce(many, .utterance(meeting: "m1", utterance: Utterance(track: .mic, t0Ms: UInt64(i), t1Ms: UInt64(i), text: "\(i)"), final: true), now: t0)
        }
        #expect(many.lines.count == MeetingSlice.linesKept && many.lines.first?.utterance.text == "5")
    }

    @Test func topicAddedReplacesById() {
        let a = Topic(id: "t1", label: "A", atMs: 1)
        let a2 = Topic(id: "t1", label: "A'", atMs: 2, cached: "c")
        let b = Topic(id: "t2", label: "B", atMs: 3)
        var s = MeetingReducer.reduce(live(), .topicAdded(meeting: "m1", topic: a), now: t0)
        s = MeetingReducer.reduce(s, .topicAdded(meeting: "m1", topic: b), now: t0)
        s = MeetingReducer.reduce(s, .topicAdded(meeting: "m1", topic: a2), now: t0)
        #expect(s.topics == [b, a2])
    }

    @Test func topicRemovedDropsTopicAndTap() {
        var s = live()
        s.topics = [Topic(id: "t1", label: "A", atMs: 1), Topic(id: "t2", label: "B", atMs: 2)]
        s.tapped = ["t1", "t2"]
        let r = MeetingReducer.reduce(s, .topicRemoved(meeting: "m1", topic: "t1", why: .viewed), now: t0)
        #expect(r.topics.map(\.id) == ["t2"] && r.tapped == ["t2"])
    }

    @Test func briefAutoAtTheTopShows() {
        var s = MeetingReducer.reduce(live(), .brief(meeting: "m1", brief: brief("b1")), now: t0)
        #expect(s.cursor == 0 && s.unseen == 0 && s.current?.id == "b1")
        s = MeetingReducer.reduce(s, .brief(meeting: "m1", brief: brief("b2", .cache)), now: t0)
        #expect(s.cursor == 1 && s.unseen == 0 && s.current?.id == "b2")
    }

    @Test func briefAutoSteppedBackQueuesAndBadges() {
        var s = MeetingReducer.reduce(live(), .brief(meeting: "m1", brief: brief("b1")), now: t0)
        s = MeetingReducer.reduce(s, .brief(meeting: "m1", brief: brief("b2")), now: t0)
        s = MeetingReducer.move(s, -1)
        #expect(s.cursor == 0)
        s = MeetingReducer.reduce(s, .brief(meeting: "m1", brief: brief("b3")), now: t0)
        s = MeetingReducer.reduce(s, .brief(meeting: "m1", brief: brief("b4", .cache)), now: t0)
        #expect(s.cursor == 0 && s.unseen == 2 && s.briefs.count == 4)
        // Stepping up eats the badge one by one; down leaves it.
        s = MeetingReducer.move(s, 1)
        #expect(s.cursor == 1 && s.unseen == 2)
        s = MeetingReducer.move(s, 1)
        #expect(s.cursor == 2 && s.unseen == 1)
        s = MeetingReducer.move(s, -1)
        #expect(s.cursor == 1 && s.unseen == 1)
        #expect(MeetingReducer.move(s, 10).cursor == 3 && MeetingReducer.move(s, -10).cursor == 0)
    }

    @Test func keyCycleWrapsAtBothEnds() {
        // 149i §N: J/↓ (−1, older) and K/↑ (+1, newer) go round; the chevrons' clamp is untouched; nothing to cycle = no change.
        var s = live()
        #expect(MeetingReducer.move(s, -1, wrap: true) == s)
        for id in ["b1", "b2", "b3"] { s = MeetingReducer.reduce(s, .brief(meeting: "m1", brief: brief(id)), now: t0) }
        #expect(s.cursor == 2)
        s = MeetingReducer.move(s, 1, wrap: true)
        #expect(s.cursor == 0, "newer past the newest wraps to the oldest")
        s = MeetingReducer.move(s, -1, wrap: true)
        #expect(s.cursor == 2, "older past the oldest wraps to the newest")
        #expect(MeetingReducer.move(s, -1, wrap: true).cursor == 1 && MeetingReducer.move(s, 1).cursor == 2)
    }

    @Test func briefTapOrRequestJumps() {
        var s = MeetingReducer.reduce(live(), .brief(meeting: "m1", brief: brief("b1")), now: t0)
        s = MeetingReducer.reduce(s, .brief(meeting: "m1", brief: brief("b2")), now: t0)
        s = MeetingReducer.move(s, -1)
        s = MeetingReducer.reduce(s, .brief(meeting: "m1", brief: brief("b3")), now: t0)
        s = MeetingReducer.markTapped(s, "t7")
        s = MeetingReducer.markTapped(s, "t7")
        #expect(s.unseen == 1 && s.tapped == ["t7"])
        let tap = MeetingReducer.reduce(s, .brief(meeting: "m1", brief: brief("b4", .tap, topic: "t7")), now: t0)
        #expect(tap.cursor == 3 && tap.unseen == 0 && tap.tapped.isEmpty)
        for src in [BriefSource.request, .deeper, .typed, .oracle] {
            let r = MeetingReducer.reduce(s, .brief(meeting: "m1", brief: brief("bx", src)), now: t0)
            #expect(r.cursor == 3 && r.unseen == 0 && r.tapped == ["t7"])
        }
    }

    @Test func listeningFollowsHeld() {
        let on = MeetingReducer.reduce(live(), .listening(meeting: "m1", held: true), now: t0)
        #expect(on.listening)
        #expect(!MeetingReducer.reduce(on, .listening(meeting: "m1", held: false), now: t0).listening)
    }

    @Test func approvalWaitingFollowsWaiting() {
        let on = MeetingReducer.reduce(live(), .approvalWaiting(meeting: "m1", waiting: true), now: t0)
        #expect(on.approvalWaiting)
        #expect(!MeetingReducer.reduce(on, .approvalWaiting(meeting: "m1", waiting: false), now: t0).approvalWaiting)
    }

    @Test func postProgressSetsPostStateAndProgress() {
        let s = MeetingReducer.reduce(live(.stopping), .postProgress(meeting: "m1", stage: .diarizing, pct: 40), now: t0)
        #expect(s.state == .post(.diarizing) && s.progress == .init(stage: .diarizing, pct: 40))
        #expect(MeetingReducer.reduce(s, .postProgress(meeting: "zz", stage: .aligning, pct: 1), now: t0) == s)
        // A Post State keeps the progress; the next State out of Post drops it.
        let kept = MeetingReducer.reduce(s, .state(meeting: "m1", state: .post(.aligning), degraded: []), now: t0)
        #expect(kept.progress != nil)
    }

    @Test func reviewReadyClearsProgress() {
        let s = MeetingReducer.reduce(live(.stopping), .postProgress(meeting: "m1", stage: .aligning, pct: 99), now: t0)
        let r = MeetingReducer.reduce(s, .reviewReady(meeting: "m1"), now: t0)
        #expect(r.state == .review && r.progress == nil)
        #expect(MeetingReducer.reduce(s, .reviewReady(meeting: "zz"), now: t0) == s)
    }

    @Test func summarizedKeepsTheSlice() {
        let s = live(.review)
        #expect(MeetingReducer.reduce(s, .summarized(meeting: "m1", title: "Standup"), now: t0) == s)
    }

    @Test func deletedEmptiesTheSlice() {
        let s = live()
        #expect(MeetingReducer.reduce(s, .deleted(meeting: "m1"), now: t0) == .empty)
        #expect(MeetingReducer.reduce(s, .deleted(meeting: "zz"), now: t0) == s)
    }

    @Test func oracleDecidingFlashes() {
        let s = MeetingReducer.reduce(live(), .oracleDeciding(meeting: "m1", batchId: "k1"), now: t0)
        #expect(s.deciding == "k1")
        #expect(MeetingReducer.reduce(live(), .oracleDeciding(meeting: "zz", batchId: "k1"), now: t0).deciding == nil)
    }

    @Test func oracleDecisionReplacesOrAppendsAndClearsDeciding() {
        var s = MeetingReducer.reduce(live(), .oracleDeciding(meeting: "m1", batchId: "k1"), now: t0)
        s = MeetingReducer.reduce(s, .oracleDecision(meeting: "m1", decision: .init(batchId: "k0", family: "skip")), now: t0)
        #expect(s.deciding == "k1" && s.oracle.map(\.batchId) == ["k0"])
        s = MeetingReducer.reduce(s, .oracleDecision(meeting: "m1", decision: .init(batchId: "k1", family: "brief")), now: t0)
        #expect(s.deciding == nil && s.oracle.map(\.batchId) == ["k0", "k1"])
        s = MeetingReducer.reduce(s, .oracleDecision(meeting: "m1", decision: .init(batchId: "k0", family: "escalate")), now: t0)
        #expect(s.oracle.map(\.family) == ["escalate", "brief"])
        for i in 0..<MeetingSlice.oracleKept {
            s = MeetingReducer.reduce(s, .oracleDecision(meeting: "m1", decision: .init(batchId: "n\(i)", family: "skip")), now: t0)
        }
        #expect(s.oracle.count == MeetingSlice.oracleKept && s.oracle.first?.batchId == "n0")
    }

    @Test func seedFromMeetingStatus() {
        let topics = [Topic(id: "t1", label: "A", atMs: 1)]
        let status = MeetingStatus(state: .paused, meeting: "m1", session: "s1", elapsedMs: 5_000, degraded: [.diskLow],
                                   cursor: "b1", topics: topics, approvalWaiting: true)
        let s = MeetingReducer.seed(status, now: t0)
        #expect(s.meeting == "m1" && s.session == "s1" && s.state == .paused && s.elapsedMs == 5_000)
        #expect(s.degraded == [.diskLow] && s.topics == topics && s.approvalWaiting && s.clockAt == t0 && s.cursor == -1)
        let lines = [Utterance(track: .mic, t0Ms: 1, t1Ms: 2, text: "x")]
        let h = MeetingReducer.hydrate(s, briefs: [brief("b1"), brief("b2")], lines: lines, cursor: "b1")
        #expect(h.cursor == 0 && h.current?.id == "b1" && h.lines.count == 1 && !h.lines[0].open)
        #expect(MeetingReducer.hydrate(s, briefs: [brief("b1"), brief("b2")], lines: [], cursor: "gone").cursor == 1)
        #expect(MeetingReducer.hydrate(s, briefs: [], lines: [], cursor: nil).cursor == -1)
    }

    @Test func elapsedExcludesPauses() {
        var s = live()
        s.elapsedMs = 60_000
        #expect(MeetingReducer.elapsed(s, now: t0 + 2.5) == 62_500)
        s.state = .paused
        #expect(MeetingReducer.elapsed(s, now: t0 + 100) == 60_000)
        #expect(MeetingReducer.mmss(62_500) == "01:02" && MeetingReducer.mmss(3_661_000) == "61:01")
    }

    // MARK: 158 §5 — the prompt (Detected / DetectionCleared / AppReleased)

    var detection: Detection { Detection(app: proc, appId: "us.zoom.xos", since: "2026-10-10T10:00:00Z") }

    @Test func detectedShowsThePromptAndStartClearsIt() {
        let s = MeetingReducer.reduce(.empty, .detected(detection), now: t0)
        #expect(s.prompt == .detected(detection) && s.meeting == nil && s.state == .idle)
        // Any meeting starting (Started, or the engine's DetectionCleared{Started}) withdraws it.
        #expect(MeetingReducer.reduce(s, .started(meeting: "m2", session: "s2", process: proc, at: "now"), now: t0).prompt == nil)
        #expect(MeetingReducer.reduce(s, .detectionCleared(appId: "us.zoom.xos", reason: .started), now: t0).prompt == nil)
    }

    @Test func detectionClearedOnlyForTheShownApp() {
        let s = MeetingReducer.reduce(.empty, .detected(detection), now: t0)
        for r in [DetectionCleared.notNow, .never, .released] {
            #expect(MeetingReducer.reduce(s, .detectionCleared(appId: "us.zoom.xos", reason: r), now: t0).prompt == nil)
        }
        #expect(MeetingReducer.reduce(s, .detectionCleared(appId: "com.apple.FaceTime", reason: .released), now: t0) == s)
        #expect(MeetingReducer.reduce(.empty, .detectionCleared(appId: "us.zoom.xos", reason: .released), now: t0) == .empty)
    }

    @Test func appReleasedAsksOnlyForTheLiveMeeting() {
        let s = MeetingReducer.reduce(live(), .appReleased(meeting: "m1", app: proc, releasedForS: 30), now: t0)
        #expect(s.prompt == .stop(app: proc, releasedForS: 30) && s.isLive)
        #expect(MeetingReducer.reduce(live(), .appReleased(meeting: "other", app: proc, releasedForS: 30), now: t0).prompt == nil)
        #expect(MeetingReducer.reduce(live(.review), .appReleased(meeting: "m1", app: proc, releasedForS: 30), now: t0).prompt == nil)
        // The 30 s re-send updates the count; the meeting ending takes the question with it.
        #expect(MeetingReducer.reduce(s, .appReleased(meeting: "m1", app: proc, releasedForS: 60), now: t0).prompt == .stop(app: proc, releasedForS: 60))
        #expect(MeetingReducer.reduce(s, .state(meeting: "m1", state: .done, degraded: []), now: t0).prompt == nil)
        #expect(MeetingReducer.reduce(s, .state(meeting: "m1", state: .paused, degraded: []), now: t0).prompt == s.prompt)
    }

    @Test func keepRecordingMutesTheReSendsUntilAFreshRelease() {
        let asked = MeetingReducer.reduce(live(), .appReleased(meeting: "m1", app: proc, releasedForS: 30), now: t0)
        let kept = MeetingReducer.keepRecording(asked)
        #expect(kept.prompt == nil && kept.keptReleasedForS == 30 && kept.isLive)
        #expect(MeetingReducer.keepRecording(live()) == live()) // nothing asked
        let resent = MeetingReducer.reduce(kept, .appReleased(meeting: "m1", app: proc, releasedForS: 60), now: t0)
        #expect(resent == kept)
        let fresh = MeetingReducer.reduce(kept, .appReleased(meeting: "m1", app: proc, releasedForS: 30), now: t0)
        #expect(fresh.prompt == .stop(app: proc, releasedForS: 30) && fresh.keptReleasedForS == nil)
    }

    @Test func seedCarriesTheDetection() {
        let status = MeetingStatus(state: .idle, detection: detection)
        #expect(MeetingReducer.seed(status, now: t0).prompt == .detected(detection))
        #expect(MeetingReducer.seed(MeetingStatus(), now: t0).prompt == nil)
    }

    // MARK: 158 §5.2 (c) / §9.4 — Upcoming

    func event(_ id: String = "ek:u1:", startIn: TimeInterval = 240) -> EventRef {
        let f = ISO8601DateFormatter()
        return EventRef(id: id, uid: id, title: "Weekly sync", start: f.string(from: t0 + startIn), end: f.string(from: t0 + startIn + 1800),
                        attendees: [Attendee(name: "Sky", isMe: true), Attendee(name: "Priya Shah"), Attendee(name: "Tom Okafor")])
    }

    @Test func upcomingShowsBeforeAMeetingAndNeverDuringOne() {
        let e = event()
        let s = MeetingReducer.reduce(.empty, .upcoming(event: e, project: "theuth", brief: ["a", "b"]), now: t0)
        #expect(s.prompt == .upcoming(event: e, project: "theuth", brief: ["a", "b"]) && s.meeting == nil)
        #expect(MeetingReducer.reduce(.empty, .upcoming(event: e, project: nil, brief: nil), now: t0).prompt == .upcoming(event: e, project: nil, brief: []))
        for st in [MeetingState.starting, .recording, .paused, .stopping] {
            #expect(MeetingReducer.reduce(live(st), .upcoming(event: e, project: nil, brief: nil), now: t0) == live(st))
        }
        // A finished meeting's slice still takes it; Started clears it (via `.empty`); a stale State{Done} of another meeting does not.
        #expect(MeetingReducer.reduce(live(.done), .upcoming(event: e, project: nil, brief: nil), now: t0).prompt != nil)
        #expect(MeetingReducer.reduce(s, .started(meeting: "m2", session: "s2", process: proc, at: "now"), now: t0).prompt == nil)
        #expect(MeetingReducer.reduce(s, .state(meeting: "old", state: .done, degraded: []), now: t0).prompt == s.prompt)
        // Past start + 5 min it is refused outright.
        #expect(MeetingReducer.reduce(.empty, .upcoming(event: event(startIn: -301), project: nil, brief: nil), now: t0) == .empty)
        #expect(MeetingReducer.reduce(.empty, .upcoming(event: event(startIn: -299), project: nil, brief: nil), now: t0).prompt != nil)
    }

    @Test func upcomingTickExpiresAtStartPlusFiveMinutes() {
        let s = MeetingReducer.reduce(.empty, .upcoming(event: event(startIn: 60), project: nil, brief: ["a"]), now: t0)
        #expect(MeetingReducer.tick(s, now: t0 + 359) == s)
        #expect(MeetingReducer.tick(s, now: t0 + 360).prompt == nil)
        let d = MeetingReducer.reduce(.empty, .detected(detection), now: t0)
        #expect(MeetingReducer.tick(d, now: t0 + 10_000) == d) // only (c) has a clock
    }

    @Test func dismissUpcomingMutesTheReSendsOfThatEvent() {
        let shown = MeetingReducer.reduce(.empty, .upcoming(event: event(), project: nil, brief: nil), now: t0)
        let gone = MeetingReducer.dismissUpcoming(shown)
        #expect(gone.prompt == nil && gone.dismissedUpcoming == "ek:u1:")
        #expect(MeetingReducer.dismissUpcoming(.empty) == .empty)
        #expect(MeetingReducer.reduce(gone, .upcoming(event: event(startIn: 120), project: nil, brief: nil), now: t0) == gone)
        let other = MeetingReducer.reduce(gone, .upcoming(event: event("ek:u2:"), project: nil, brief: nil), now: t0)
        #expect(other.prompt == .upcoming(event: event("ek:u2:"), project: nil, brief: []))
    }

    @Test func upcomingAndDetectedForTheSameEventMergeIntoB() {
        let e = event()
        var withEvent = detection; withEvent.event = EventMatch(event: e, score: 0.9)
        var merged = withEvent; merged.brief = ["a"]; merged.project = "theuth"
        // Upcoming first, then Detected: the detection takes the pane and keeps the brief + project.
        let c = MeetingReducer.reduce(.empty, .upcoming(event: e, project: "theuth", brief: ["a"]), now: t0)
        #expect(MeetingReducer.reduce(c, .detected(withEvent), now: t0).prompt == .detected(merged))
        // Detected first, then Upcoming: the brief lands on the shown detection; the engine's own values win.
        let b = MeetingReducer.reduce(.empty, .detected(withEvent), now: t0)
        #expect(MeetingReducer.reduce(b, .upcoming(event: e, project: "theuth", brief: ["a"]), now: t0).prompt == .detected(merged))
        var own = withEvent; own.brief = ["engine"]; own.project = "p"
        #expect(MeetingReducer.reduce(c, .detected(own), now: t0).prompt == .detected(own))
        // An Upcoming without bullets, then the detection, then the bullets: the empty brief does not shadow the real one.
        let bare = MeetingReducer.reduce(.empty, .upcoming(event: e, project: "theuth", brief: nil), now: t0)
        let then = MeetingReducer.reduce(bare, .detected(withEvent), now: t0)
        #expect(MeetingReducer.reduce(then, .upcoming(event: e, project: "theuth", brief: ["a"]), now: t0).prompt == .detected(merged))
        // An unrelated detection (no event, or another event) is untouched and keeps the pane.
        let a = MeetingReducer.reduce(.empty, .detected(detection), now: t0)
        #expect(MeetingReducer.reduce(a, .upcoming(event: e, project: "theuth", brief: ["a"]), now: t0) == a)
        #expect(MeetingReducer.reduce(c, .detected(detection), now: t0).prompt == .detected(detection))
    }

    @Test func upcomingLinesFollowTheMock() {
        #expect(MeetingReducer.upcomingLine(event(startIn: 270), now: t0) == "In 5 min · Weekly sync")
        #expect(MeetingReducer.upcomingLine(event(startIn: 60), now: t0) == "In 1 min · Weekly sync")
        #expect(MeetingReducer.upcomingLine(event(startIn: 0), now: t0) == "Now · Weekly sync")
        #expect(MeetingReducer.upcomingLine(event(startIn: -90), now: t0) == "Now · Weekly sync")
        #expect(MeetingReducer.upcomingLine(EventRef(id: "e", uid: "u", title: "Sync", start: "soon", end: "later"), now: t0) == "Sync")
        #expect(MeetingReducer.upcomingSource(project: "theuth", attendees: event().attendees) == "From the last meetings in theuth · attendees Priya Shah, Tom Okafor")
        #expect(MeetingReducer.upcomingSource(project: nil, attendees: event().attendees) == "attendees Priya Shah, Tom Okafor")
        #expect(MeetingReducer.upcomingSource(project: "theuth", attendees: []) == "From the last meetings in theuth")
        #expect(MeetingReducer.upcomingSource(project: nil, attendees: [Attendee(name: "Me", isMe: true)]) == nil)
    }

    @Test func eventLineFormatsTitleAndTimes() {
        let e = EventRef(id: "e", uid: "u", title: "Weekly sync", start: "2026-10-10T10:00:00Z", end: "2026-10-10T10:30:00Z")
        let line = MeetingReducer.eventLine(e)
        #expect(line.hasPrefix("Weekly sync · ") && line.contains("–") && line.count > "Weekly sync · ".count + 5)
        #expect(MeetingReducer.eventLine(EventRef(id: "e", uid: "u", title: "Sync", start: "soon", end: "later")) == "Sync")
        let frac = EventRef(id: "e", uid: "u", title: "S", start: "2026-10-10T10:00:00.000Z", end: "2026-10-10T10:30:00.000Z")
        #expect(MeetingReducer.eventLine(frac) == MeetingReducer.eventLine(EventRef(id: "e", uid: "u", title: "S", start: "2026-10-10T10:00:00Z", end: "2026-10-10T10:30:00Z")))
    }

    // MARK: 173 — capture health

    @Test func captureAppliesToTheCurrentMeetingAndClearsWhenNotLive() {
        let bad = CaptureStatus(mic: .ok, system: .recovering(reason: .stalled, since: "2026-11-03T10:00:00Z"))
        let s = MeetingReducer.reduce(live(), .capture(meeting: "m1", capture: bad), now: t0)
        #expect(s.capture == bad)
        #expect(MeetingReducer.reduce(live(), .capture(meeting: "other", capture: bad), now: t0).capture == .ok)
        #expect(MeetingReducer.reduce(s, .state(meeting: "m1", state: .paused, degraded: []), now: t0).capture == bad)
        #expect(MeetingReducer.reduce(s, .state(meeting: "m1", state: .done, degraded: []), now: t0).capture == .ok)
        // Started carries the app name; the status seed carries the capture.
        #expect(MeetingReducer.reduce(.empty, .started(meeting: "m2", session: "s2", process: proc, at: "now"), now: t0).app == "Zoom")
        #expect(MeetingReducer.seed(MeetingStatus(state: .recording, meeting: "m1", capture: bad), now: t0).capture == bad)
    }

    @Test func captureWarningFollowsTheWebWording() {
        let since = Date(timeIntervalSince1970: 1_793_700_000)
        let iso = ISO8601DateFormatter().string(from: since)
        let c = Calendar.current.dateComponents([.hour, .minute], from: since)
        let hm = String(format: "%02d:%02d", c.hour!, c.minute!)
        #expect(MeetingReducer.captureWarning(.ok, app: "Zoom") == nil)
        #expect(MeetingReducer.captureWarning(CaptureStatus(mic: .ok, system: .recovering(reason: .stalled, since: iso)), app: "Zoom")
                == "Not hearing Zoom since \(hm) — reconnecting…")
        #expect(MeetingReducer.captureWarning(CaptureStatus(mic: .lost(reason: .permissionDenied, since: iso), system: .ok), app: nil)
                == "Not hearing your microphone since \(hm) — Microphone permission is off (System Settings → Privacy & Security). Retrying…")
        #expect(MeetingReducer.captureWarning(CaptureStatus(mic: .lost(reason: .silent, since: iso), system: .degraded(reason: .failed)), app: nil)
                == "the meeting app's audio tap failed; recording all system audio instead · Not hearing your microphone since \(hm). Still silent — check the input device or Microphone permission. Retrying…")
        #expect(MeetingReducer.captureBadge(CaptureStatus(mic: .lost(reason: .silent, since: iso), system: .recovering(reason: .stalled, since: iso))) == "reconnecting audio")
        #expect(MeetingReducer.captureBadge(CaptureStatus(mic: .lost(reason: .permissionDenied, since: iso), system: .ok)) == "no mic permission")
        #expect(MeetingReducer.captureBadge(.ok) == nil)
    }

    @Test func stripBadgesLeaveThePermissionToTheCaptureBadge() {
        let since = "2026-11-03T10:00:00Z"
        var s = MeetingReducer.reduce(live(), .capture(meeting: "m1", capture: CaptureStatus(mic: .ok, system: .lost(reason: .permissionDenied, since: since))), now: t0)
        s = MeetingReducer.reduce(s, .state(meeting: "m1", state: .recording, degraded: [.permissionDenied("AudioCapture"), .micNoAec]), now: t0)
        #expect(MeetingReducer.stripBadges(s) == ["headphones recommended", "no system audio permission"])
        // The permission came back: the capture badge clears itself; the stale degradation never shows.
        s = MeetingReducer.reduce(s, .capture(meeting: "m1", capture: .ok), now: t0)
        #expect(MeetingReducer.stripBadges(s) == ["headphones recommended"])
        #expect(!MeetingReducer.stripBadges(s).contains { $0.contains("permission") })
        s.listening = true; s.approvalWaiting = true
        #expect(MeetingReducer.stripBadges(s) == ["headphones recommended", "approval waiting", "listening…"])
        #expect(MeetingReducer.badge(.systemAudioFallback(reason: "Silent", ended: false)) == "system audio fallback")
        #expect(MeetingReducer.badge(.systemAudioFallback(reason: "Silent", ended: true)) == nil)
        // The live fallback is said once: by the capture badge.
        s = MeetingReducer.reduce(s, .capture(meeting: "m1", capture: CaptureStatus(mic: .ok, system: .degraded(reason: .silent))), now: t0)
        s = MeetingReducer.reduce(s, .state(meeting: "m1", state: .recording, degraded: [.systemAudioFallback(reason: "Silent", ended: false)]), now: t0)
        #expect(MeetingReducer.stripBadges(s).filter { $0 == "system audio fallback" }.count == 1)
    }

    @Test func capturePillNamesTheUnheardTrack() {
        let since = "2026-11-03T10:00:00Z"
        #expect(MeetingReducer.capturePill(.ok, app: "zoom.us") == nil)
        #expect(MeetingReducer.capturePill(CaptureStatus(mic: .ok, system: .recovering(reason: .stalled, since: since)), app: "zoom.us") == "Not hearing zoom.us")
        #expect(MeetingReducer.capturePill(CaptureStatus(mic: .ok, system: .lost(reason: .permissionDenied, since: since)), app: nil) == "Not hearing the meeting app")
        #expect(MeetingReducer.capturePill(CaptureStatus(mic: .lost(reason: .silent, since: since), system: .degraded(reason: .failed)), app: "zoom.us") == "Recording all system audio")
        #expect(MeetingReducer.capturePill(CaptureStatus(mic: .lost(reason: .silent, since: since), system: .ok), app: "zoom.us") == "Not hearing your microphone")
        #expect(MeetingReducer.capturePill(CaptureStatus(mic: .recovering(reason: .stalled, since: since), system: .ok), app: nil) == "Not hearing your microphone")
    }
}
