import Foundation
import TheuthKit

// 149 W1 fills the bodies (design 149 §3.1, §5.1). C fixes the shapes.

/// The four separate pills (D1): inbox · agents · actions · more. Geometry in points.
public struct RailPills: Equatable, Sendable {
    public enum Kind: Equatable, Sendable, CaseIterable { case inbox, agents, actions, more }

    public struct Pill: Equatable, Sendable, Identifiable {
        public var kind: Kind
        /// The inbox count (gold when > 0) · the agent dots (≤ `maxDots`, then `+N`) · the level bars.
        public var count: Int
        public var dots: [Mark]
        public var overflow: Int
        /// 0–1 per band, newest right (`DictationLevel.levels`); actions pill only.
        public var levels: [Float]
        public var hovered: Bool
        public var id: Kind { kind }

        public init(kind: Kind, count: Int = 0, dots: [Mark] = [], overflow: Int = 0, levels: [Float] = [], hovered: Bool = false) {
            self.kind = kind; self.count = count; self.dots = dots; self.overflow = overflow; self.levels = levels; self.hovered = hovered
        }
    }

    public var pills: [Pill]

    public static let height: Double = 52
    public static let gap: Double = 12
    public static let maxDots = 12

    public init(pills: [Pill]) { self.pills = pills }

    /// The pills for a `Rail` (`RailModel.derive`, unchanged) and the live level bars.
    public static func derive(rail: Rail, levels: [Float], hovered: Kind?) -> RailPills {
        fatalError("149 W1")
    }
}
