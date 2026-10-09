import CoreGraphics
import Foundation
import TheuthKit

// 149 W2 (design 149 §3.7): samples → per-display runs. AppKit-free: a display is its UUID string plus its frame in
// global logical points (`NSScreen.frame` with the y flipped), the engine's coordinate system (`pointer.rs:3-5`).

public struct OverlayDisplay: Equatable, Sendable, Identifiable {
    public var id: String
    /// Global logical frame, top-left origin.
    public var frame: CGRect
    public init(id: String, frame: CGRect) { self.id = id; self.frame = frame }
}

/// One contiguous pen-down run on one display, in that display's local points, and the ring when the newest sample
/// landed on it. A pen-up sample or a display change ends a run, so a display may own several (`OverlayModel.paths`).
public struct OverlayPath: Equatable, Sendable {
    public var display: String
    public var points: [CGPoint]
    /// The ring (14 pt) sits here when the newest sample is on this display.
    public var ring: CGPoint?
    public init(display: String, points: [CGPoint], ring: CGPoint?) { self.display = display; self.points = points; self.ring = ring }
}

public enum OverlayModel {
    public static let strokeWidth: Double = 6
    public static let ringDiameter: Double = 14
    public static let alpha: Double = 0.85

    /// Pen-down samples split by the display that contains them, in that display's local points; a sample on no display
    /// (a seam, a display that just vanished) is skipped. The ring follows the newest sample, pen up or down, on the last run
    /// of its display (an empty run is added when that display has none yet).
    public static func paths(samples: [PointerSample], displays: [OverlayDisplay]) -> [OverlayPath] {
        var out: [OverlayPath] = []
        var open: (display: String, index: Int)?
        var newest: (display: String, point: CGPoint)?
        for s in samples {
            let p = CGPoint(x: Double(s.x), y: Double(s.y))
            guard let d = displays.first(where: { $0.frame.contains(p) }) else { continue }
            let local = CGPoint(x: p.x - d.frame.minX, y: p.y - d.frame.minY)
            if s.pen {
                if let o = open, o.display == d.id {
                    out[o.index].points.append(local)
                } else {
                    out.append(OverlayPath(display: d.id, points: [local], ring: nil))
                    open = (d.id, out.count - 1)
                }
            } else {
                open = nil
            }
            newest = (d.id, local)
        }
        if let newest {
            if let i = out.lastIndex(where: { $0.display == newest.display }) { out[i].ring = newest.point }
            else { out.append(OverlayPath(display: newest.display, points: [], ring: newest.point)) }
        }
        return out
    }
}
