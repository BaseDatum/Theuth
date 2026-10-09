import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W2: samples → per-display paths in that display's logical points, ring at the newest point (§3.7).
struct OverlayModelTests {
    @Test(.disabled("149 W2")) func samplesSplitByTheDisplayThatContainsThem() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func pointsAreLocalToTheirDisplay() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func ringSitsAtTheNewestSampleOnly() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func penUpSamplesBreakTheTrail() {
        Issue.record("149 W2 writes this test")
    }

    @Test(.disabled("149 W2")) func noDisplayNoPath() {
        Issue.record("149 W2 writes this test")
    }
}
