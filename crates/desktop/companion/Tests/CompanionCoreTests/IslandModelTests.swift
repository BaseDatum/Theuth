import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W3 / 149i: visibility (D2 — the Tauri original's rule, no linger), the attached layout + collapse (D1), snap-back,
// frame memory + validation (§3.6).
struct IslandModelTests {
    let notch = CGRect(x: 1100, y: 1403, width: 200, height: 37)

    func slice(_ state: MeetingState) -> MeetingSlice { var s = MeetingSlice.empty; s.meeting = "m"; s.state = state; return s }

    @Test func visibleWhileLive() {
        for st in [MeetingState.starting, .recording, .paused, .stopping] {
            #expect(IslandModel.live(slice(st)))
            #expect(IslandModel.visible(slice(st), alwaysShow: false))
        }
    }

    @Test func visibleWhenAlwaysShow() {
        #expect(IslandModel.visible(.empty, alwaysShow: true))
        #expect(IslandModel.visible(slice(.done), alwaysShow: true))
    }

    @Test func hiddenTheMomentTheMeetingIsNotLive() {
        // Done/Failed/Idle and every post-meeting state: no linger — ⌃M closes it, the engine's notes/summary never show it.
        for st in [MeetingState.idle, .review, .summarizing, .done, .failed, .post(.aligning), .other("New")] {
            #expect(!IslandModel.live(slice(st)))
            #expect(!IslandModel.visible(slice(st), alwaysShow: false))
        }
    }

    @Test func attachedLayoutIsOpenWhileLiveUnlessCollapsed() {
        #expect(IslandModel.attached(live: true, collapsed: false) == .open)
        #expect(IslandModel.attached(live: true, collapsed: true) == .closed)
        #expect(IslandModel.attached(live: false, collapsed: false) == .closed) // the idle wordmark ("Always show")
    }

    @Test func briefKeysCycleOutsideTheBoxAndOnlyArrowsInAnEmptyBox() {
        // 149i §N: J/K and ↑/↓ cycle while the box is not focused; in the box J/K type (never cycle), ↑/↓ cycle only when empty.
        #expect(IslandModel.cyclesBrief(arrow: false, boxFocused: false, textEmpty: false))
        #expect(IslandModel.cyclesBrief(arrow: true, boxFocused: false, textEmpty: false))
        #expect(!IslandModel.cyclesBrief(arrow: false, boxFocused: true, textEmpty: true), "J in the empty box types")
        #expect(!IslandModel.cyclesBrief(arrow: false, boxFocused: true, textEmpty: false))
        #expect(IslandModel.cyclesBrief(arrow: true, boxFocused: true, textEmpty: true), "↓ in the empty box cycles")
        #expect(!IslandModel.cyclesBrief(arrow: true, boxFocused: true, textEmpty: false), "↓ with text moves the caret")
        let t = KeyTable()
        #expect(t.action(for: Chord(chars: "j", keyCode: 38), in: .island) == .islandNext)
        #expect(t.action(for: Chord(chars: "K", keyCode: 40, modifiers: .shift), in: .island) == .islandPrevious)
        #expect(t.action(for: .down, in: .island) == .islandNext && t.action(for: .up, in: .island) == .islandPrevious)
    }

    @Test func dictationPillLivesInTheIslandArea() {
        // 149i §P: centred on the island anchor, under the notch / in the fallback slot / below a live island; never at the pointer.
        let visible = CGRect(x: 0, y: 0, width: 2560, height: 1403)
        let size = CGSize(width: 320, height: 40)
        let underNotch = IslandModel.pillFrame(size: size, anchor: notch, notch: true, island: nil, visible: visible)
        #expect(underNotch.midX == notch.midX && underNotch.maxY == notch.minY - 6 && underNotch.size == size)
        let slot = CGRect(x: 1130, y: 1380, width: 300, height: 30) // the fallback pill under the menu bar
        let inSlot = IslandModel.pillFrame(size: size, anchor: slot, notch: false, island: nil, visible: visible)
        #expect(inSlot.midX == slot.midX && inSlot.maxY == slot.maxY, "no notch: the pill fills the fallback slot")
        let island = CGRect(x: 920, y: 1160, width: 560, height: 280) // the live island's open pane hanging from the notch
        let below = IslandModel.pillFrame(size: size, anchor: notch, notch: true, island: island, visible: visible)
        #expect(below.maxY == island.minY - 6 && below.midX == notch.midX && !below.intersects(island), "a live island pushes it below itself")
        // Kept inside the visible frame on a narrow display.
        let narrow = IslandModel.pillFrame(size: CGSize(width: 320, height: 40), anchor: CGRect(x: 0, y: 760, width: 200, height: 37), notch: true, island: nil,
                                           visible: CGRect(x: 0, y: 0, width: 400, height: 760))
        #expect(narrow.minX == 8)
    }

    @Test func collapseIsRememberedForTheCurrentMeetingOnly() {
        var m = IslandModel()
        m.meetingChanged(to: "m1")
        m.toggleCollapsed(meeting: "m1")
        #expect(m.collapsed)
        m.meetingChanged(to: "m1") // the same meeting's next slice: still collapsed
        #expect(m.collapsed)
        m.toggleCollapsed(meeting: "m1")
        #expect(!m.collapsed) // a second click opens again
        m.toggleCollapsed(meeting: "m1")
        m.meetingChanged(to: "m2") // a new meeting forgets it
        #expect(!m.collapsed)
        m.toggleCollapsed(meeting: "m2")
        m.meetingChanged(to: nil) // Deleted → no meeting: forgotten too
        #expect(!m.collapsed)
    }

    @Test func aGripClickOnTheOpenPaneSnapsBackNotDetaches() {
        // The open pane hangs 300 pt below the notch: its centre is far from the notch's, its top-centre is not.
        var m = IslandModel()
        let pane = CGRect(x: notch.midX - 280, y: notch.maxY - 300, width: 560, height: 300)
        m.dragEnded(frame: pane, notch: notch, display: "D")
        #expect(m.state == .closed && m.frames["D"] == nil)
        m.dragEnded(frame: pane.offsetBy(dx: 0, dy: -41), notch: notch, display: "D")
        #expect(m.state == .detached(frame: pane.offsetBy(dx: 0, dy: -41)))
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
