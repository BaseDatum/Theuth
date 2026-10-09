import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W1: per kind — header, status line, bubbles, options, footer verbs, `ifWrong` (§3.3, Revision 1 rows 2, 3, 6, 7).
struct CardAnatomyTests {
    @Test(.disabled("149 W1")) func headerCarriesPositionCountAndGlance() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func statusLineSaysJustNowUnderFortyFiveSeconds() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func statusLineProjectFallbackChain() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func bubblesComeFromItemContextWhenPresent() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func bubblesFallBackToTheInterimPairWithoutContext() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func optionsMarkRecommendedAndArrow() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func stopOnlyWhenTheAgentRunsOrWaits() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func stopAbsentOnFinishedCards() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func clearOnlyWhenClearable() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func askCardsSnoozeNotClear() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func ifWrongRenderedWhenPresent() {
        Issue.record("149 W1 writes this test")
    }
}
