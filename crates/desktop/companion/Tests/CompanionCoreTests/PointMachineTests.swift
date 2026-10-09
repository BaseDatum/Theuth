import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W2: the 17 tests of `crates/desktop/src/point/machine.rs:158-480` ported one-to-one.
struct PointMachineTests {
    @Test(.disabled("149 W2")) func idlePressStartsAndArms() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func refusedPressAnswersRefuseNotStart() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func autoRepeatPressIsIgnored() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func armedReleaseBeforeHoldIsTap1() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func armedReleaseAfterHoldSends() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func armedTickMovesToHoldingOnlyAfterHold() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func holdingReleaseSendsOrDiscardsShort() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func secondPressInsideWindowLocks() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func lateSecondPressClosesTheTapAndStartsAnew() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func tap1TickAfterWindowDiscardsWithHint() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func lockedPressSendsOrDiscardsShort() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func escInEveryActiveState() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func cancelInEveryActiveState() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func panelDoneOnlyMattersInSendingAndReplying() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func replyingPressStartsANewQuestion() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func fullHoldThenDoubleTapSequence() {
        Issue.record("149 W2 writes this test")
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
