import CoreGraphics
import Foundation
import TheuthKit

// 149 W2 fills the bodies (design 149 §3.7): samples → per-display paths. C fixes the shapes. AppKit-free: a
// display is its UUID string plus its frame in global logical points (`NSScreen.frame` with the y flipped).

public struct OverlayDisplay: Equatable, Sendable, Identifiable {
    public var id: String
    /// Global logical frame, top-left origin.
    public var frame: CGRect
    public init(id: String, frame: CGRect) { self.id = id; self.frame = frame }
}

/// What one display's overlay strokes: the trail in that display's local points and the ring at the newest point.
public struct OverlayPath: Equatable, Sendable, Identifiable {
    public var display: String
    public var points: [CGPoint]
    /// The ring (14 pt) sits here when the newest sample is on this display.
    public var ring: CGPoint?
    public var id: String { display }
    public init(display: String, points: [CGPoint], ring: CGPoint?) { self.display = display; self.points = points; self.ring = ring }
}

public enum OverlayModel {
    public static let strokeWidth: Double = 6
    public static let ringDiameter: Double = 14
    public static let alpha: Double = 0.85

    /// Pen-down samples split by the display that contains them, in that display's local points.
    public static func paths(samples: [PointerSample], displays: [OverlayDisplay]) -> [OverlayPath] {
        fatalError("149 W2")
    }
}
