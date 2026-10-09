import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W3: one test per `MeetingEvent` variant mirroring `UI/store/meeting.ts:156-220`, plus the brief-stack rule (`:181-190`).
struct MeetingReducerTests {
    @Test(.disabled("149 W3")) func startedResetsToARecordingSlice() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func stateUpdatesAndEndsTheCuesWhenNotLive() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func stateForAnotherMeetingIsIgnored() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func levelKeepsTheMeters() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func utterancePartialReplacesTheOpenLineOfItsTrack() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func utteranceFinalWithSpeakerUpdatesInPlace() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func topicAddedReplacesById() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func topicRemovedDropsTopicAndTap() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func briefAutoAtTheTopShows() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func briefAutoSteppedBackQueuesAndBadges() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func briefTapOrRequestJumps() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func listeningFollowsHeld() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func approvalWaitingFollowsWaiting() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func postProgressSetsPostStateAndProgress() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func reviewReadyClearsProgress() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func summarizedKeepsTheSlice() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func deletedEmptiesTheSlice() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func oracleDecidingFlashes() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func oracleDecisionReplacesOrAppendsAndClearsDeciding() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func seedFromMeetingStatus() {
        Issue.record("149 W3 writes this test")
    }

    @Test(.disabled("149 W3")) func elapsedExcludesPauses() {
        Issue.record("149 W3 writes this test")
    }
}
