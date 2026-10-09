import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W3: visibility (D10, Revision 1 row 13), snap-back, frame memory + validation (§3.6).
struct IslandModelTests {
    let t0 = Date(timeIntervalSince1970: 1_000_000)
    let notch = CGRect(x: 1100, y: 1403, width: 200, height: 37)

    func slice(_ state: MeetingState) -> MeetingSlice { var s = MeetingSlice.empty; s.meeting = "m"; s.state = state; return s }

    @Test func visibleWhileLive() {
        for st in [MeetingState.starting, .recording, .paused, .stopping] {
            #expect(IslandModel.visible(slice(st), lastActivity: nil, alwaysShow: false, now: t0))
        }
    }

    @Test func visibleTenMinutesAfterTheLastTopicOrBrief() {
        let s = slice(.done)
        #expect(IslandModel.visible(s, lastActivity: t0 - 9 * 60, alwaysShow: false, now: t0))
        #expect(IslandModel.visible(s, lastActivity: t0 - IslandModel.linger, alwaysShow: false, now: t0))
        #expect(!IslandModel.visible(s, lastActivity: t0 - IslandModel.linger - 1, alwaysShow: false, now: t0))
    }

    @Test func visibleWhenAlwaysShow() {
        #expect(IslandModel.visible(.empty, lastActivity: nil, alwaysShow: true, now: t0))
    }

    @Test func hiddenOtherwise() {
        for st in [MeetingState.idle, .review, .summarizing, .done, .failed, .post(.aligning), .other("New")] {
            #expect(!IslandModel.visible(slice(st), lastActivity: nil, alwaysShow: false, now: t0))
        }
    }

    @Test func dragWithinFortyPointsSnapsBackClosed() {
        var m = IslandModel(state: .detached(frame: .zero), frames: ["D": CGRect(x: 1, y: 1, width: 1, height: 1)])
        // Same size as the notch, centre 39 pt away.
        m.dragEnded(frame: notch.offsetBy(dx: 39, dy: 0), notch: notch, display: "D")
        #expect(m.state == .closed && m.frames["D"] == nil)
        m.dragEnded(frame: notch.offsetBy(dx: 28, dy: 28), notch: notch, display: "D") // hypot 39.6
        #expect(m.state == .closed)
    }

    @Test func dragBeyondFortyPointsDetachesAndRemembers() {
        var m = IslandModel()
        let far = notch.offsetBy(dx: 0, dy: -41)
        m.dragEnded(frame: far, notch: notch, display: "D")
        #expect(m.state == .detached(frame: far) && m.frames["D"] == far)
        #expect(m.frames["E"] == nil)
    }

    @Test func offScreenFrameIsRejected() {
        let screen = CGRect(x: 0, y: 0, width: 2560, height: 1410)
        let m = IslandModel(frames: ["D": CGRect(x: 2600, y: 100, width: 420, height: 44),
                                     "E": CGRect(x: 2400, y: 100, width: 420, height: 44)])
        #expect(m.frame(for: "D", visibleFrames: [screen]) == nil) // wholly off the right edge
        #expect(m.frame(for: "E", visibleFrames: [screen]) == CGRect(x: 2400, y: 100, width: 420, height: 44)) // partly on
        #expect(m.frame(for: "D", visibleFrames: []) == nil)
        #expect(m.frame(for: "missing", visibleFrames: [screen]) == nil)
    }

    @Test func framesKeyedByUUIDSurviveATopologyChange() {
        // Two displays; the external's frame is saved in its own coordinates, so moving it in the arrangement
        // (or unplugging the other) changes nothing about what is remembered.
        var m = IslandModel()
        let ext = CGRect(x: 40, y: 900, width: 420, height: 300)
        m.dragEnded(frame: ext, notch: notch, display: "EXT-UUID")
        m.dragEnded(frame: CGRect(x: 10, y: 10, width: 420, height: 300), notch: notch, display: "BUILTIN-UUID")
        let extVisible = CGRect(x: 0, y: 0, width: 2560, height: 1410)
        #expect(m.frame(for: "EXT-UUID", visibleFrames: [extVisible]) == ext)
        // A smaller resolution on the same display: the frame no longer fits → rejected, the default is used.
        #expect(m.frame(for: "EXT-UUID", visibleFrames: [CGRect(x: 0, y: 0, width: 1280, height: 800)]) == nil)
        #expect(m.frames.count == 2)
    }
}
