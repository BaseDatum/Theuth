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

    /// 149i §N — whether a next/previous key (J/K or ↑/↓) cycles the briefs: always while the request box is not focused; in
    /// the box J and K type letters (never cycle) and the arrows cycle only while the text is empty (else they move the caret).
    public static func cyclesBrief(arrow: Bool, boxFocused: Bool, textEmpty: Bool) -> Bool {
        !boxFocused || (arrow && textEmpty)
    }

    /// 149i §P — the point-and-say dictation pill and the answer card live in the island area for the whole gesture, never at
    /// the pointer: hanging under the hardware notch, or filling the fallback slot under the menu bar (`anchor` is what
    /// `IslandGeometry.anchor` gives the island on the pointer's display); directly below a live attached island there
    /// (`island`, its drawn frame), never overlapping it. Centred on the anchor, kept inside `visible` (bottom-left coordinates).
    public static func pillFrame(size: CGSize, anchor: CGRect, notch: Bool, island: CGRect?, visible: CGRect) -> CGRect {
        let gap: CGFloat = 6
        let top: CGFloat
        if let i = island { top = i.minY - gap }
        else if notch { top = anchor.minY - gap }
        else { top = anchor.maxY }
        let x = min(max(visible.minX + 8, anchor.midX - size.width / 2), visible.maxX - size.width - 8)
        let y = max(visible.minY + 8, top - size.height)
        return CGRect(x: x, y: y, width: size.width, height: size.height)
    }

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
