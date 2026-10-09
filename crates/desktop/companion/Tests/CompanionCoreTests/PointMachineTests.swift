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
