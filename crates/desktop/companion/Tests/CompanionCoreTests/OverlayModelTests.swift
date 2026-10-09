import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W2: samples → per-display runs in that display's logical points, ring at the newest sample (§3.7).
struct OverlayModelTests {
    /// A 1000×600 main display with a 800×600 one to its right, top edges aligned (top-left origin, as the engine).
    static let a = OverlayDisplay(id: "A", frame: CGRect(x: 0, y: 0, width: 1000, height: 600))
    static let b = OverlayDisplay(id: "B", frame: CGRect(x: 1000, y: 0, width: 800, height: 600))
    static let displays = [a, b]

    func sample(_ x: Float, _ y: Float, pen: Bool = true, t: UInt32 = 0) -> PointerSample { PointerSample(tMs: t, x: x, y: y, pen: pen) }

    @Test func samplesSplitByTheDisplayThatContainsThem() {
        let paths = OverlayModel.paths(samples: [sample(10, 10), sample(20, 20), sample(1500, 100), sample(1600, 120)], displays: Self.displays)
        #expect(paths.map(\.display) == ["A", "B"])
        #expect(paths[0].points == [CGPoint(x: 10, y: 10), CGPoint(x: 20, y: 20)])
        #expect(paths[1].points.count == 2)
        // The right edge belongs to the next display (`x < maxX`, as `locate`/`contains` in the shell and the engine).
        let edge = OverlayModel.paths(samples: [sample(999.5, 0), sample(1000, 0)], displays: Self.displays)
        #expect(edge.map(\.display) == ["A", "B"])
    }

    @Test func pointsAreLocalToTheirDisplay() {
        let lower = OverlayDisplay(id: "C", frame: CGRect(x: -500, y: 600, width: 500, height: 300))
        let paths = OverlayModel.paths(samples: [sample(1500, 100), sample(-250, 750)], displays: Self.displays + [lower])
        #expect(paths[0] == OverlayPath(display: "B", points: [CGPoint(x: 500, y: 100)], ring: nil))
        #expect(paths[1] == OverlayPath(display: "C", points: [CGPoint(x: 250, y: 150)], ring: CGPoint(x: 250, y: 150)))
    }

    @Test func ringSitsAtTheNewestSampleOnly() {
        let paths = OverlayModel.paths(samples: [sample(10, 10), sample(1500, 100), sample(1510, 110)], displays: Self.displays)
        #expect(paths[0].ring == nil)
        #expect(paths[1].ring == CGPoint(x: 510, y: 110))
        // Back on A: the ring moves with it; B keeps its trail without the ring.
        let back = OverlayModel.paths(samples: [sample(10, 10), sample(1500, 100), sample(30, 30)], displays: Self.displays)
        #expect(back.map(\.ring) == [nil, nil, CGPoint(x: 30, y: 30)])
        #expect(back.map(\.display) == ["A", "B", "A"])
    }

    @Test func penUpSamplesBreakTheTrail() {
        let paths = OverlayModel.paths(samples: [sample(10, 10), sample(20, 20), sample(30, 30, pen: false), sample(40, 40)], displays: Self.displays)
        #expect(paths.map(\.points) == [[CGPoint(x: 10, y: 10), CGPoint(x: 20, y: 20)], [CGPoint(x: 40, y: 40)]])
        #expect(paths.map(\.ring) == [nil, CGPoint(x: 40, y: 40)])
        // Pen up at the end (annotate = mouse, button released): no new run, the ring still follows the pointer.
        let up = OverlayModel.paths(samples: [sample(10, 10), sample(50, 50, pen: false)], displays: Self.displays)
        #expect(up == [OverlayPath(display: "A", points: [CGPoint(x: 10, y: 10)], ring: CGPoint(x: 50, y: 50))])
        // Pen up on a display with no run yet: an empty run carries the ring.
        let onlyUp = OverlayModel.paths(samples: [sample(1200, 10, pen: false)], displays: Self.displays)
        #expect(onlyUp == [OverlayPath(display: "B", points: [], ring: CGPoint(x: 200, y: 10))])
    }

    @Test func noDisplayNoPath() {
        #expect(OverlayModel.paths(samples: [sample(10, 10)], displays: []).isEmpty)
        #expect(OverlayModel.paths(samples: [], displays: Self.displays).isEmpty)
        // A sample in a seam (below A, left of nothing) is skipped; the trail continues on either side.
        let seam = OverlayModel.paths(samples: [sample(10, 10), sample(10, 700), sample(20, 20)], displays: Self.displays)
        #expect(seam == [OverlayPath(display: "A", points: [CGPoint(x: 10, y: 10), CGPoint(x: 20, y: 20)], ring: CGPoint(x: 20, y: 20))])
    }
}
