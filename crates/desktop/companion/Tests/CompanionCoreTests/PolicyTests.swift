import Testing
import Foundation
import TheuthKit
@testable import CompanionCore

// `#expect` captures its operands as immutable values, so every mutating reducer call is hoisted into a `let`.

private let t0 = Date(timeIntervalSince1970: 1_791_448_800)
private func it(_ id: String, _ kind: AttentionKind, agent: String = "w2", detail: String? = nil) -> AttentionItem {
    AttentionItem(id: id, kind: kind, title: "title \(id)", detail: detail, atMs: 1_791_448_000_000, agent: agent,
                  approval: kind == .approval ? "ap-\(id)" : nil)
}
private func at(_ s: TimeInterval) -> Date { t0.addingTimeInterval(s) }

/// A policy primed with an empty list so every later item is "new".
private func primed(_ prefs: NotificationPrefs = NotificationPrefs()) -> NotificationPolicy {
    var p = NotificationPolicy(prefs: prefs)
    _ = p.update(items: [], now: at(-1))
    return p
}

struct PolicyClassTests {
    @Test func everyKindHasItsRow() {
        #expect([AttentionKind.approval, .ask, .decision, .secret, .handoff].allSatisfy { NotificationPolicy.classify(it("x", $0)) == .blocking })
        #expect([AttentionKind.review, .hire, .finished, .blocked].allSatisfy { NotificationPolicy.classify(it("x", $0)) == .deferrable })
        #expect(NotificationPolicy.classify(it("b", .budget, detail: "at 80 %: the cap pauses it")) == .deferrable)
        #expect(NotificationPolicy.classify(it("b", .budget, detail: "paused at the cap; un-pause to override for the month")) == .error)
        #expect(NotificationPolicy.classify(it("n", .node)) == .error)
        #expect([AttentionKind.show, .resume, .other("later")].allSatisfy { NotificationPolicy.classify(it("x", $0)) == .never })
    }

    @Test func blockingIsImmediateActiveSilent() throws {
        var p = primed()
        let out = p.update(items: [it("ask:1", .ask), it("show:1", .show)], now: t0)
        #expect(out.post.count == 1)
        let b = try #require(out.post.first)
        #expect(b.id == "ask:1" && b.category == .question && b.level == .active && b.sound == false && b.thread == "w2")
        #expect(out.withdraw.isEmpty)
    }

    @Test func errorIsActiveWithSoundAndTimeSensitiveOnlyWhenAvailable() {
        var p = primed()
        let plain = p.update(items: [it("node:1", .node)], now: t0)
        #expect(plain.post.map(\.level) == [.active] && plain.post.map(\.sound) == [true])
        var prefs = NotificationPrefs()
        prefs.timeSensitiveAvailable = true
        p = primed(prefs)
        let ts = p.update(items: [it("budget:ops", .budget, detail: "paused at the cap")], now: t0)
        #expect(ts.post.map(\.level) == [.timeSensitive] && ts.post.map(\.category) == [.error])
        prefs.soundOnErrors = false
        p = primed(prefs)
        let quiet = p.update(items: [it("node:2", .node)], now: t0)
        #expect(quiet.post.map(\.sound) == [false])
    }

    @Test func firstSnapshotIsSeededNotPosted() {
        var p = NotificationPolicy()
        let seed = p.update(items: [it("ask:old", .ask), it("review:old", .review)], now: t0)
        #expect(seed.isEmpty)
        let out = p.update(items: [it("ask:old", .ask), it("review:old", .review), it("ask:new", .ask)], now: at(1))
        #expect(out.post.map(\.id) == ["ask:new"], "only the genuinely new item posts")
    }
}

struct PolicyDeferralTests {
    @Test func heldUntilBreakpointThenOneBatch() throws {
        var p = primed()
        let a = p.update(items: [it("review:1", .review)], now: t0)
        let b2 = p.update(items: [it("review:1", .review), it("finished:1", .finished, agent: "w1")], now: at(10))
        #expect(a.isEmpty && b2.isEmpty && p.heldCount == 2)
        let early = p.tick(now: at(60))
        #expect(early.isEmpty, "the cap has not elapsed")
        let out = p.breakpoint(now: at(70))
        #expect(out.post.count == 1)
        let b = try #require(out.post.first)
        #expect(b.category == .batch && b.level == .passive && b.body == "1 for later · 1 finished")
        #expect(Set(b.itemIds) == ["review:1", "finished:1"])
        #expect(p.heldCount == 0)
    }

    @Test func singleHeldItemKeepsItsOwnCategory() {
        var p = primed()
        _ = p.update(items: [it("review:1", .review)], now: t0)
        let out = p.breakpoint(now: at(5))
        #expect(out.post.map(\.category) == [.review] && out.post.map(\.level) == [.passive])
    }

    @Test func twoMinuteCapReleasesWithoutBreakpoint() {
        var p = primed()
        _ = p.update(items: [it("review:1", .review)], now: t0)
        let before = p.tick(now: at(119))
        let after = p.tick(now: at(120))
        #expect(before.isEmpty && after.post.map(\.id) == ["review:1"])
    }

    @Test func steadyStateOneDeferrablePerTwoMinutes() {
        var p = primed()
        _ = p.update(items: [it("review:1", .review)], now: t0)
        let first = p.breakpoint(now: at(1))
        #expect(first.post.count == 1)
        _ = p.update(items: [it("review:1", .review), it("review:2", .review)], now: at(30))
        let inWindow = p.breakpoint(now: at(40))
        #expect(inWindow.isEmpty, "held even at a breakpoint inside the rate window")
        let edge = p.tick(now: at(120))
        let past = p.tick(now: at(121))
        #expect(edge.isEmpty && past.post.map(\.id) == ["review:2"], "the pending breakpoint releases once the window passes")
    }

    @Test func perClassModesOverrideTheTable() {
        var prefs = NotificationPrefs()
        prefs.blocking = .atBreak
        prefs.deferrable = .immediate
        prefs.error = .off
        var p = primed(prefs)
        let out = p.update(items: [it("ask:1", .ask), it("review:1", .review), it("node:1", .node)], now: t0)
        #expect(out.post.map(\.id) == ["review:1"] && out.post.map(\.level) == [.passive])
        #expect(p.heldCount == 1)
        let released = p.breakpoint(now: at(1))
        #expect(released.post.map(\.id) == ["ask:1"])
    }
}

struct PolicyFloodPauseWithdrawTests {
    @Test func floodPostsOneSummaryAndNoPerItemBanners() {
        var p = primed()
        var items = (1...11).map { it("ask:\($0)", .ask, agent: "w\($0)") }
        let enter = p.update(items: items, now: t0)
        #expect(enter.post.map(\.id) == [Banner.floodId])
        #expect(enter.post.first?.body == "11 agents need you — open the panel")
        #expect(p.inFlood)
        items.append(it("ask:12", .ask))
        let more = p.update(items: items, now: at(1))
        #expect(more.isEmpty, "no per-item banners while flooded")
        let refreshed = p.breakpoint(now: at(2))
        #expect(refreshed.post.map(\.body) == ["12 agents need you — open the panel"], "the count refreshes at a breakpoint")
        items.append(it("node:1", .node))
        let err = p.update(items: items, now: at(3))
        #expect(err.post.map(\.id) == ["node:1"], "errors still get through")
        let drained = p.update(items: Array(items.prefix(5)), now: at(4))
        #expect(drained.withdraw.contains(Banner.floodId) && drained.withdraw.contains("node:1") && !p.inFlood)
    }

    @Test func answeredElsewhereWithdrawsItsBanner() throws {
        var p = primed()
        _ = p.update(items: [it("ask:1", .ask), it("ask:2", .ask)], now: t0)
        let out = p.update(items: [it("ask:2", .ask)], now: at(1))
        #expect(out.withdraw == ["ask:1"] && out.post.isEmpty)
        // A held item that disappears is dropped silently; a batch is withdrawn once every item is gone.
        _ = p.update(items: [it("ask:2", .ask), it("review:1", .review), it("review:2", .review)], now: at(2))
        let batch = try #require(p.breakpoint(now: at(3)).post.first)
        let partial = p.update(items: [it("ask:2", .ask), it("review:2", .review)], now: at(4))
        let gone = p.update(items: [it("ask:2", .ask)], now: at(5))
        #expect(partial.withdraw.isEmpty && gone.withdraw == [batch.id])
    }

    @Test func pauseDeliversNothingAndEndsOnTick() {
        var p = primed()
        p.pause(until: at(3600))
        let a = p.update(items: [it("ask:1", .ask), it("review:1", .review)], now: t0)
        let b = p.breakpoint(now: at(1))
        let c = p.engine(connected: false, now: at(2))
        #expect(a.isEmpty && b.isEmpty && c.isEmpty)
        #expect(p.isPaused(now: at(3599)) && !p.isPaused(now: at(3600)))
        _ = p.tick(now: at(3600))
        #expect(p.pausedUntil == nil)
        let after = p.update(items: [it("ask:1", .ask), it("ask:2", .ask)], now: at(3601))
        #expect(after.post.map(\.id) == ["ask:2"], "the item that arrived during the pause was seen")
    }

    @Test func engineLostIsOneErrorBannerWithdrawnOnReconnect() {
        var p = primed()
        let lost = p.engine(connected: false, now: t0)
        #expect(lost.post.map(\.id) == [Banner.engineId] && lost.post.map(\.sound) == [true])
        let again = p.engine(connected: false, now: at(1))
        let refresh = p.update(items: [], now: at(2))
        #expect(again.isEmpty && refresh.withdraw.isEmpty, "not repeated; an item refresh does not withdraw it")
        let back = p.engine(connected: true, now: at(3))
        #expect(back.withdraw == [Banner.engineId])
    }
}

struct BreakpointDetectorTests {
    @Test func appSwitchFiresOncePerChangeAndIgnoresSelf() {
        var d = BreakpointDetector(own: "io.theuth.mac.companion")
        let r = [d.feed(.appActivated("io.theuth.mac")), d.feed(.appActivated("com.apple.Safari")),
                 d.feed(.appActivated("com.apple.Safari")), d.feed(.appActivated("io.theuth.mac.companion")),
                 d.feed(.appActivated("io.theuth.mac"))]
        #expect(r == [false, true, false, false, true], "seed · switch · same app · our own panel · switch")
    }

    @Test func idleFiresOncePerStretch() {
        var d = BreakpointDetector(own: nil)
        let r = [d.feed(.idle(19.9)), d.feed(.idle(20)), d.feed(.idle(21)), d.feed(.idle(90)), d.feed(.idle(0.3)), d.feed(.idle(20.5))]
        #expect(r == [false, true, false, false, false, true], "once per pause; typing resumed → a new pause fires again")
    }
}

struct DoubleTapDetectorTests {
    private typealias Step = (Modifiers, TimeInterval)
    private let o: Modifiers = .option
    private let n: Modifiers = []

    private func run(_ steps: [Step], cancelBefore: Int? = nil) -> [Bool] {
        var d = DoubleTapDetector()
        return steps.enumerated().map { i, s in
            if i == cancelBefore { d.cancel() }
            return d.feed(s.0, at: s.1)
        }
    }

    @Test func doubleTap() { #expect(run([(o, 0), (n, 0.08), (o, 0.18), (n, 0.26)]) == [false, false, false, true]) }
    @Test func tapThenHold() { #expect(run([(o, 0), (n, 0.08), (o, 0.18), (n, 1.0)]).allSatisfy { !$0 }) }
    @Test func longHoldThenTap() { #expect(run([(o, 0), (n, 0.60), (o, 0.70), (n, 0.75)]).allSatisfy { !$0 }) }
    @Test func chordReleaseThenTap() {
        #expect(run([(o, 0), ([.option, .command], 0.05), (o, 0.10), (n, 0.15), (o, 0.20), (n, 0.25)]).allSatisfy { !$0 })
    }
    @Test func twoTapsTooFarApart() { #expect(run([(o, 0), (n, 0.05), (o, 0.40), (n, 0.45)]).allSatisfy { !$0 }) }
    @Test func thirdTapCompletesWithTheSecond() {
        #expect(run([(o, 0), (n, 0.05), (o, 0.40), (n, 0.45), (o, 0.55), (n, 0.60)]).last == true)
    }
    @Test func autoRepeatDuplicateIgnored() { #expect(run([(o, 0), (o, 0.03), (n, 0.08), (o, 0.18), (n, 0.26)]).last == true) }
    @Test func keyBetweenTapsCancels() {
        #expect(run([(o, 0), (n, 0.05), (o, 0.15), (n, 0.20)], cancelBefore: 2).allSatisfy { !$0 })
    }
    @Test func shiftBetweenTapsCancels() {
        #expect(run([(o, 0), (n, 0.05), (.shift, 0.10), (n, 0.15), (o, 0.20), (n, 0.25)]).allSatisfy { !$0 })
    }
    @Test func capsLockIsMaskedByTheSource() {
        // The AppKit source drops `.capsLock` before feeding; the detector only ever sees the five relevant bits.
        #expect(run([(o, 0), (n, 0.08), (o, 0.18), (n, 0.26)]).last == true)
    }
}

struct SettingsPersistenceTests {
    @Test func roundTripAndDefaults() throws {
        let suite = "io.theuth.mac.companion.tests.\(UUID().uuidString)"
        let d = try #require(UserDefaults(suiteName: suite))
        defer { d.removePersistentDomain(forName: suite) }
        #expect(CompanionSettings.load(from: d) == CompanionSettings(), "empty defaults = the design's defaults")
        var s = CompanionSettings()
        s.railSide = "left"; s.railPresence = .alwaysSliver; s.doubleTapOption = true; s.undoDelay = 7
        s.keys.set(Chord(chars: "n", keyCode: 45), for: .next)
        s.notify.blocking = .atBreak; s.notify.deferrable = .off; s.notify.soundOnErrors = false; s.launchAtLogin = true
        s.save(to: d)
        var back = CompanionSettings.load(from: d)
        #expect(back.undoDelay == 5, "clamped 0–5")
        back.undoDelay = 7
        #expect(back == s)
        // W1's keys are the ones the Store and the rail already read.
        #expect(d.string(forKey: "railSide") == "left" && d.double(forKey: "undoDelay") == 5)
        #expect(String(decoding: s.loginItemJSON, as: UTF8.self) == "{ \"launch_at_login\": true }\n")
    }

    @Test func w2CollapseToggleMigratesToPresence() throws {
        let suite = "io.theuth.mac.companion.tests.\(UUID().uuidString)"
        let d = try #require(UserDefaults(suiteName: suite))
        defer { d.removePersistentDomain(forName: suite) }
        d.set(false, forKey: "railCollapse")
        #expect(CompanionSettings.load(from: d).railPresence == .expanded)
        d.set(true, forKey: "railCollapse")
        #expect(CompanionSettings.load(from: d).railPresence == .sliverWhenQuiet)
        d.set("alwaysSliver", forKey: "railPresence")
        #expect(CompanionSettings.load(from: d).railPresence == .alwaysSliver, "the new key wins")
    }
}

// 155 §7.6 / §11: the engine decides whether (`alert`), the policy decides how.
private func alerted(_ id: String, _ kind: AttentionKind, _ cls: AlertClass, episode: UInt64 = 1, blanked: Bool = false, session: String = "s1") -> AttentionItem {
    AttentionItem(id: id, kind: kind, title: "title \(id)", atMs: 1_791_448_000_000, agent: "w2", session: session,
                  alert: blanked ? nil : Alert(id: "\(id)@\(episode)", cls: cls, why: "why \(id)", atMs: episode), engineDecides: true)
}

struct PolicyAlertTests {
    @Test func oneBannerPerAlertIdWithWhyAsBody() throws {
        var p = primed()
        let first = p.update(items: [alerted("ask:1", .ask, .needsYou), alerted("show:1", .show, .needsYou, blanked: true)], now: t0)
        let b = try #require(first.post.first)
        #expect(first.post.count == 1 && b.id == "ask:1@1" && b.body == "why ask:1" && b.category == .question && b.level == .active && b.itemIds == ["ask:1"])
        let again = p.update(items: [alerted("ask:1", .ask, .needsYou)], now: at(5))
        #expect(again.isEmpty, "the same episode never posts twice")
        // Blanked by the engine (the session is in view there) → nothing; back with the same id → still nothing.
        let blanked = p.update(items: [alerted("ask:1", .ask, .needsYou, blanked: true)], now: at(6))
        let back = p.update(items: [alerted("ask:1", .ask, .needsYou)], now: at(7))
        #expect(blanked.isEmpty && back.isEmpty)
        // A new episode (the item left and returned) is a new id.
        let gone = p.update(items: [], now: at(8))
        #expect(gone.withdraw == ["ask:1@1"])
        let episode2 = p.update(items: [alerted("ask:1", .ask, .needsYou, episode: 9)], now: at(9))
        #expect(episode2.post.map(\.id) == ["ask:1@9"])
    }

    @Test func engineClassRulesWhenItDecided() {
        var p = primed()
        // A finished alert is deferrable: held, not posted; a needs-you one on a kind the table calls deferrable is blocking.
        let out = p.update(items: [alerted("finished:s1:t", .finished, .finished), alerted("review:1", .review, .needsYou)], now: t0)
        #expect(out.post.map(\.id) == ["review:1@1"] && p.heldCount == 1)
        // An item the engine decided against (alert absent on the wire = blanked) never falls back to the table.
        let quiet = p.update(items: [alerted("approval:9", .approval, .needsYou, blanked: true)], now: at(1))
        #expect(quiet.post.isEmpty)
    }

    @Test func pausedPostsNothing() {
        var p = primed()
        p.pause(until: at(3600))
        let during = p.update(items: [alerted("ask:1", .ask, .needsYou)], now: at(10))
        #expect(during.isEmpty && p.isPaused(now: at(10)))
        _ = p.tick(now: at(3601))
        #expect(!p.isPaused(now: at(3601)))
        let after = p.update(items: [alerted("ask:1", .ask, .needsYou), alerted("ask:2", .ask, .needsYou)], now: at(3602))
        #expect(after.post.map(\.id) == ["ask:2@1"], "what arrived during the pause stays quiet; what arrives after posts")
    }

    @Test func olderEngineFallsBackToClassify() throws {
        var p = primed()
        let out = p.update(items: [it("ask:1", .ask), it("review:1", .review)], now: t0)
        let b = try #require(out.post.first)
        #expect(out.post.count == 1 && b.id == "ask:1" && b.body == "title ask:1" && p.heldCount == 1)
        #expect(NotificationPolicy.classOf(it("review:1", .review)) == .deferrable)
        #expect(NotificationPolicy.classOf(alerted("review:1", .review, .needsYou)) == .blocking)
        #expect(NotificationPolicy.classOf(alerted("ask:1", .ask, .finished)) == .deferrable)
    }
}
