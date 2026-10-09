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

    public static let snapBack: Double = 40
    /// Visible for this long after the last topic/brief once the meeting ends (Revision 1 row 13).
    public static let linger: TimeInterval = 10 * 60
    /// A screen without a notch: the pill's size under the menu bar.
    public static let fallbackPill = CGSize(width: 300, height: 24)

    public init(state: IslandState = .closed, frames: [String: CGRect] = [:], alwaysShow: Bool = false) {
        self.state = state; self.frames = frames; self.alwaysShow = alwaysShow
    }

    /// D10: live meeting, or ≤ `linger` after the last topic/brief, or "Always show".
    public static func visible(_ slice: MeetingSlice, lastActivity: Date?, alwaysShow: Bool, now: Date) -> Bool {
        fatalError("149 W3")
    }

    /// A drag ended at `frame`: within `snapBack` of the notch → `.closed`, else `.detached` and remembered.
    public mutating func dragEnded(frame: CGRect, notch: CGRect, display: String) {
        fatalError("149 W3")
    }

    /// The remembered frame for `display`, rejected when it intersects none of `visibleFrames`.
    public func frame(for display: String, visibleFrames: [CGRect]) -> CGRect? {
        fatalError("149 W3")
    }
}
