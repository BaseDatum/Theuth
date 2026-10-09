import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W1: rows and host groups from `list_activity` ∪ `agents_list` ∪ `list_sessions` + `mesh_status` (§3.2, §6 E7 fallback).
struct AgentsModelTests {
    @Test(.disabled("149 W1")) func emptyOwnerAndSelfIdAreThisMac() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func remoteOwnerGroupsUnderThePeerNameAndIsNotLive() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func unknownPeerIdIsNamedByItsId() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func childrenFoldUnderTheirRootWithACount() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func desksAppearAsRows() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func labelIsTitleOrNameNeverTheHarnessKey() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func firstWorkingRowIsTheOpeningScrollTarget() {
        Issue.record("149 W1 writes this test")
    }

    @Test(.disabled("149 W1")) func sessionsStatusCountsLabelTheGroups() {
        Issue.record("149 W1 writes this test")
    }
}
