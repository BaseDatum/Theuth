import CoreGraphics
import Foundation
import TheuthKit

// 149 W3 fills the bodies (design 149 §3.6, D9, D10, Revision 1 rows 12–13). C fixes the shapes. AppKit-free:
// frames are `CGRect`s in the screen's logical points; displays are keyed by their UUID string.

public enum IslandState: Equatable, Sendable {
    /// The notch-hugging pill.
    case closed
    /// The brief/transcript/request pane under the pill.
    case open
    /// Dragged away from the notch; `frame` is remembered per display.
    case detached(frame: CGRect)
}

public struct IslandModel: Equatable, Sendable {
    public var state: IslandState
    /// Display UUID → the detached frame the user left it at.
    public var frames: [String: CGRect]
    public var alwaysShow: Bool
    /// 149i D1: the user clicked the strip/pill to collapse the open pane; remembered for that meeting only.
    public private(set) var collapsed = false
    private var collapsedMeeting: String?

    public static let snapBack: Double = 40
    /// A screen without a notch: the pill's size under the menu bar.
    public static let fallbackPill = CGSize(width: 300, height: 24)

    public init(state: IslandState = .closed, frames: [String: CGRect] = [:], alwaysShow: Bool = false) {
        self.state = state; self.frames = frames; self.alwaysShow = alwaysShow
    }

    /// Starting/Recording/Paused/Stopping — the strip is up from the first second to the last (`Island.tsx:73`).
    public static func live(_ slice: MeetingSlice) -> Bool {
        switch slice.state {
        case .starting, .recording, .paused, .stopping: return true
        default: return false
        }
    }

    /// 149i D2 — the Tauri original's rule: shown iff live, or "Always show" (the idle wordmark). No linger: Done/Failed/
    /// Idle and the post-meeting states hide it at once.
    public static func visible(_ slice: MeetingSlice, alwaysShow: Bool) -> Bool { alwaysShow || live(slice) }

    /// 149i D1 — the attached layout: live → the strip + pane, unless the user collapsed it; otherwise the pill.
    public static func attached(live: Bool, collapsed: Bool) -> IslandState { live && !collapsed ? .open : .closed }

    /// A click on the pill/strip: collapsed ↔ open, for `meeting`.
    public mutating func toggleCollapsed(meeting: String?) { collapsed.toggle(); collapsedMeeting = meeting }

    /// Another meeting (or none) forgets the collapse.
    public mutating func meetingChanged(to meeting: String?) {
        guard meeting != collapsedMeeting else { return }
        collapsed = false; collapsedMeeting = meeting
    }

    /// A drag ended with the island's content at `frame`: its top-centre within `snapBack` of the anchor's top-centre (where
    /// it hangs when attached, open or closed) → back to the notch, else `.detached` and remembered. The top-centre, not the
    /// centre: the open pane's centre is already far below the notch's, which would detach on a mere grip click.
    public mutating func dragEnded(frame: CGRect, notch: CGRect, display: String) {
        let d = hypot(frame.midX - notch.midX, frame.maxY - notch.maxY)
        if d <= Self.snapBack {
            state = .closed
            frames[display] = nil
        } else {
            state = .detached(frame: frame)
            frames[display] = frame
        }
    }

    /// The remembered frame for `display`, rejected when it intersects none of `visibleFrames`.
    public func frame(for display: String, visibleFrames: [CGRect]) -> CGRect? {
        guard let f = frames[display] else { return nil }
        return visibleFrames.contains { $0.intersects(f) } ? f : nil
    }
}
