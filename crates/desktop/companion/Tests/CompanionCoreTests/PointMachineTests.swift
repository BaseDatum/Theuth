import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W2: the 16 behavioural tests of `crates/desktop/src/point/machine.rs:158-467` ported one-to-one (the 17th,
// `state_serialises_tagged_snake_case`, has no Swift equivalent — the state never crosses a wire here).
struct PointMachineTests {
    typealias S = PointMachine.State
    typealias E = PointMachine.Event
    typealias F = PointMachine.Effect
    static let hold = PointMachine.holdMs, doubleTap = PointMachine.doubleTapMs, minNote = PointMachine.minNoteMs
    static let hint = PointMachine.hint

    /// `step` with no standing refusal; the pair compares as one value.
    func s(_ st: S, _ e: E, _ now: UInt64) -> (S, [F]) { PointMachine.step(st, e, now: now, refused: nil) }

    func expectStep(_ got: (S, [F]), _ state: S, _ effects: [F], _ note: String = "", sourceLocation: SourceLocation = #_sourceLocation) {
        #expect(got.0 == state, "state \(note)", sourceLocation: sourceLocation)
        #expect(got.1 == effects, "effects \(note)", sourceLocation: sourceLocation)
    }

    @Test func idlePressStartsAndArms() {
        expectStep(s(.idle, .pressed, 10), .armed(t0: 10), [.start])
        for e in [E.released, .esc, .tick(now: 5), .panelDone, .cancel] {
            expectStep(s(.idle, e, 10), .idle, [], "\(e)")
        }
    }

    @Test func refusedPressAnswersRefuseNotStart() {
        for st in [S.idle, .replying] {
            expectStep(PointMachine.step(st, .pressed, now: 10, refused: "meeting"), st, [.refuse(reason: "meeting")])
        }
        // Only a press is refused; nothing else changes.
        expectStep(PointMachine.step(.holding(t0: 0), .released, now: 1000, refused: "meeting"),
                   .sending(t0: 0), [.stop(reason: "release")])
    }

    @Test func autoRepeatPressIsIgnored() {
        for st in [S.armed(t0: 0), .holding(t0: 0)] {
            expectStep(s(st, .pressed, 50), st, [], "\(st)")
        }
        // Locked's press is the send, not a repeat — covered below; Sending ignores presses too.
        expectStep(s(.sending(t0: 0), .pressed, 50), .sending(t0: 0), [])
    }

    @Test func armedReleaseBeforeHoldIsTap1() {
        expectStep(s(.armed(t0: 0), .released, Self.hold - 1), .tap1(t0: 0, released: Self.hold - 1), [])
    }

    @Test func armedReleaseAfterHoldSends() {
        expectStep(s(.armed(t0: 0), .released, Self.minNote), .sending(t0: 0), [.stop(reason: "release")])
        // A release past HOLD_MS but under MIN_NOTE_MS (a late hold tick) is the same short note Holding would discard.
        expectStep(s(.armed(t0: 0), .released, Self.hold + 10), .idle, [.discard(reason: "short", hint: Self.hint)])
    }

    @Test func armedTickMovesToHoldingOnlyAfterHold() {
        expectStep(s(.armed(t0: 0), .tick(now: Self.hold), Self.hold), .holding(t0: 0), [])
        expectStep(s(.armed(t0: 0), .tick(now: 50), Self.hold - 1), .armed(t0: 0), [])
    }

    @Test func holdingReleaseSendsOrDiscardsShort() {
        expectStep(s(.holding(t0: 0), .released, Self.minNote), .sending(t0: 0), [.stop(reason: "release")])
        expectStep(s(.holding(t0: 0), .released, Self.minNote - 1), .idle, [.discard(reason: "short", hint: Self.hint)])
    }

    @Test func secondPressInsideWindowLocks() {
        expectStep(s(.tap1(t0: 0, released: 100), .pressed, 100 + Self.doubleTap), .locked(t0: 0), [.showLock])
    }

    @Test func lateSecondPressClosesTheTapAndStartsAnew() {
        expectStep(s(.tap1(t0: 0, released: 100), .pressed, 100 + Self.doubleTap + 1),
                   .armed(t0: 401), [.discard(reason: "double-tap-missed", hint: Self.hint), .start])
    }

    @Test func tap1TickAfterWindowDiscardsWithHint() {
        let st = S.tap1(t0: 0, released: 100)
        expectStep(s(st, .tick(now: 401), 100 + Self.doubleTap), st, [])
        expectStep(s(st, .tick(now: 401), 100 + Self.doubleTap + 1),
                   .idle, [.discard(reason: "double-tap-missed", hint: "Hold to talk · double-tap to lock")])
        expectStep(s(st, .released, 150), st, [])
    }

    @Test func lockedPressSendsOrDiscardsShort() {
        expectStep(s(.locked(t0: 0), .pressed, Self.minNote - 1), .idle, [.discard(reason: "short", hint: Self.hint)])
        expectStep(s(.locked(t0: 0), .pressed, Self.minNote), .sending(t0: 0), [.stop(reason: "tap")])
        expectStep(s(.locked(t0: 0), .released, 50), .locked(t0: 0), [])
        expectStep(s(.locked(t0: 0), .tick(now: 50), 50), .locked(t0: 0), [])
    }

    @Test func escInEveryActiveState() {
        for st in [S.armed(t0: 0), .holding(t0: 0), .locked(t0: 0)] {
            expectStep(s(st, .esc, 50), .idle, [.discard(reason: "esc", hint: Self.hint)], "\(st)")
        }
        for st in [S.tap1(t0: 0, released: 10), .sending(t0: 0)] {
            expectStep(s(st, .esc, 50), .idle, [.discard(reason: "esc", hint: nil)], "\(st)")
        }
        expectStep(s(.replying, .esc, 50), .idle, [.conceal])
    }

    @Test func cancelInEveryActiveState() {
        for st in [S.armed(t0: 0), .holding(t0: 0), .tap1(t0: 0, released: 10), .locked(t0: 0)] {
            expectStep(s(st, .cancel, 50), .idle, [.discard(reason: "cancel", hint: nil)], "\(st)")
        }
        expectStep(s(.sending(t0: 0), .cancel, 50), .idle, [])
        expectStep(s(.replying, .cancel, 50), .idle, [.conceal])
    }

    @Test func panelDoneOnlyMattersInSendingAndReplying() {
        expectStep(s(.sending(t0: 0), .panelDone, 50), .replying, [])
        expectStep(s(.replying, .panelDone, 50), .idle, [.conceal])
        for st in [S.armed(t0: 0), .holding(t0: 0), .tap1(t0: 0, released: 10), .locked(t0: 0)] {
            expectStep(s(st, .panelDone, 50), st, [], "\(st)")
        }
    }

    @Test func replyingPressStartsANewQuestion() {
        expectStep(s(.replying, .pressed, 9000), .armed(t0: 9000), [.start])
        for e in [E.released, .tick(now: 1)] {
            expectStep(s(.replying, e, 9000), .replying, [], "\(e)")
        }
    }

    @Test func fullHoldThenDoubleTapSequence() {
        // Hold: press, tick → Holding, release at 1.2 s → Stop.
        var (st, fx) = s(.idle, .pressed, 0)
        #expect(fx == [.start])
        (st, _) = s(st, .tick(now: Self.hold), Self.hold)
        #expect(st == .holding(t0: 0))
        (st, fx) = s(st, .released, 1200)
        expectStep((st, fx), .sending(t0: 0), [.stop(reason: "release")])
        (st, _) = s(st, .panelDone, 1300)
        (st, fx) = s(st, .panelDone, 7300)
        expectStep((st, fx), .idle, [.conceal])
        // Lock: press, release at 100, press at 250 → Locked, press at 2000 → Stop{tap}.
        (st, _) = s(.idle, .pressed, 0)
        (st, _) = s(st, .released, 100)
        (st, fx) = s(st, .pressed, 250)
        expectStep((st, fx), .locked(t0: 0), [.showLock])
        (st, _) = s(st, .released, 300)
        (st, fx) = s(st, .pressed, 2000)
        expectStep((st, fx), .sending(t0: 0), [.stop(reason: "tap")])
    }

    /// C's contract: the 96 §4.1 constants as the shell has them (`machine.rs:6-18`).
    @Test func constantsMatchTheShell() {
        #expect(PointMachine.holdMs == 200)
        #expect(PointMachine.doubleTapMs == 300)
        #expect(PointMachine.minNoteMs == 800)
        #expect(PointMachine.sampleHz == 60)
        #expect(PointMachine.doneMs == 6_000)
    }
}

/// 149f: the hot key's key-down/key-up through `VoiceKeyGate` into the machine — the controller's `keyDown`/`keyUp` as a pure
/// driver with a fake clock (ms). What Carbon delivers while ⌃S is held: one key-down, then a key-down every `repeatMs`.
struct VoiceKeyGateTests {
    static let repeatMs: UInt64 = 30

    /// The gate in front of the machine; `starts` counts `.start` effects, `refusals` `.refuse`.
    struct Driver {
        var gate = VoiceKeyGate()
        var state = PointMachine.State.idle
        var starts = 0, refusals = 0
        var refused: String?

        mutating func keyDown(_ now: UInt64) {
            guard gate.press(now: now) else { return }
            step(.pressed, now)
        }

        mutating func keyUp(_ now: UInt64) {
            guard gate.release() else { return }
            step(.released, now)
        }

        mutating func step(_ e: PointMachine.Event, _ now: UInt64) {
            let (next, fx) = PointMachine.step(state, e, now: now, refused: refused)
            state = next
            starts += fx.filter { $0 == .start }.count
            refusals += fx.filter { if case .refuse = $0 { return true } else { return false } }.count
            discards += fx.compactMap { if case .discard(let r, let h) = $0 { return (r, h) } else { return nil } }
        }
        var discards: [(reason: String, hint: String?)] = []

        /// Hold from `t0` for `ms`: the key-down, auto-repeats every `repeatMs`, then the key-up. Returns the release time.
        mutating func hold(from t0: UInt64, ms: UInt64, between: (inout Driver, UInt64) -> Void = { _, _ in }) -> UInt64 {
            keyDown(t0)
            var t = t0
            while t + repeatMs < t0 + ms {
                t += repeatMs
                between(&self, t)
                keyDown(t)
            }
            keyUp(t0 + ms)
            return t0 + ms
        }
    }

    @Test func oneHoldIsOneGesture() {
        var d = Driver()
        let released = d.hold(from: 0, ms: 1500) { d, t in if t >= PointMachine.holdMs, case .armed = d.state { d.step(.tick(now: t), t) } }
        #expect(d.starts == 1)
        #expect(d.state == .sending(t0: 0))
        #expect(released == 1500)
        // The next physical press is a new gesture.
        d.step(.cancel, 1600)
        d.keyDown(2000)
        #expect(d.starts == 2)
    }

    /// The report: a refusal ends the gesture; before 149f the next auto-repeat re-armed it (start/refuse/cancel at 30 ms).
    @Test func refusalEndsTheGestureForTheRestOfTheHold() {
        var d = Driver()
        _ = d.hold(from: 0, ms: 1500) { d, t in
            // The engine's "already dictating" lands right after the first start: the controller cancels.
            if d.starts == 1, case .armed = d.state { d.step(.cancel, t) }
        }
        #expect(d.starts == 1)
        #expect(d.state == .idle)
        // Released: a real press re-arms.
        d.keyDown(2000)
        #expect(d.starts == 2)
    }

    /// A standing refusal (a live meeting) answers once per physical press, not once per auto-repeat.
    @Test func standingRefusalAnswersOncePerPress() {
        var d = Driver()
        d.refused = "talking"
        _ = d.hold(from: 0, ms: 1500)
        #expect(d.refusals == 1)
        #expect(d.starts == 0)
        d.keyDown(2000)
        #expect(d.refusals == 2)
    }

    @Test func lockedHoldDoesNotSendOnAutoRepeat() {
        var d = Driver()
        d.keyDown(0); d.keyUp(100) // tap
        d.keyDown(250) // → locked
        #expect(d.state == .locked(t0: 0))
        var t: UInt64 = 250
        for _ in 0..<40 { t += Self.repeatMs; d.keyDown(t) } // held for 1.2 s
        #expect(d.state == .locked(t0: 0))
        d.keyUp(t)
        d.keyDown(t + 500) // the real tap sends
        #expect(d.state == .sending(t0: 0))
    }

    /// 149g F1: ⎋ while the chord is physically held (it reaches the controller as ⌃⎋ — the second registration) cancels at once
    /// with the hint; the auto-repeats that follow and the key-up change nothing; the next physical press is a new gesture.
    @Test func escDuringTheHoldCancelsAndTheRestOfTheHoldIsIgnored() {
        var d = Driver()
        var stateAtEsc: PointMachine.State?
        _ = d.hold(from: 0, ms: 1500) { d, t in
            if t >= PointMachine.holdMs, case .armed = d.state { d.step(.tick(now: t), t) }
            if t >= 500, stateAtEsc == nil { stateAtEsc = d.state; d.step(.esc, t) }
        }
        #expect(stateAtEsc == .holding(t0: 0))
        #expect(d.starts == 1 && d.state == .idle)
        #expect(d.discards.count == 1 && d.discards[0].reason == "esc" && d.discards[0].hint == PointMachine.hint)
        d.keyDown(2000)
        #expect(d.starts == 2, "released and pressed again: a new gesture")
    }

    @Test func escCancelsAnArmedOrLockedHoldToo() {
        var d = Driver()
        d.keyDown(0); d.keyDown(30) // armed, one auto-repeat, no hold tick yet
        d.step(.esc, 60)
        #expect(d.state == .idle && d.discards.last?.hint == PointMachine.hint)
        d.keyUp(100)
        d.keyDown(500); d.keyUp(600); d.keyDown(800) // tap, tap → locked
        #expect(d.state == .locked(t0: 500))
        for t in stride(from: 830, through: 1400, by: 30) { d.keyDown(UInt64(t)) } // the second tap held down
        d.step(.esc, 1400)
        #expect(d.state == .idle && d.discards.last?.reason == "esc" && d.discards.last?.hint == PointMachine.hint)
        d.keyUp(1500)
        #expect(d.state == .idle && d.starts == 2)
    }

    @Test func lostKeyUpIsNotADeadKey() {
        var g = VoiceKeyGate()
        let presses = [g.press(now: 0), g.press(now: 30),
                       g.press(now: VoiceKeyGate.lostKeyUpMs - 1 + 30), // inside the slowest auto-repeat
                       g.press(now: VoiceKeyGate.lostKeyUpMs * 2 + 30)] // no release ever came: a new press
        #expect(presses == [true, false, false, true])
        let releases = [g.release(), g.release()] // the second is a stray key-up
        #expect(releases == [true, false])
    }
}
