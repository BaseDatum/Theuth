import Foundation
import TheuthKit

// 149 W1 (design 149 §3.1, D1, §5.1). C fixed the shapes.

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

    public subscript(_ kind: Kind) -> Pill { pills.first { $0.kind == kind }! }

    /// The whole stack: four pills and three gaps.
    public static var stackHeight: Double { Double(Kind.allCases.count) * height + Double(Kind.allCases.count - 1) * gap }

    /// What the collapsed 4 × 120 sliver says for VoiceOver: the counts survive the collapse.
    public var sliverLabel: String {
        let needs = self[.inbox].count
        let agents = self[.agents].dots.filter { !$0.isNeedsYou }.count + self[.agents].overflow
        var parts = ["Theuth rail"]
        parts.append(needs == 0 ? "nothing needs you" : needs == 1 ? "1 needs you" : "\(needs) need you")
        if agents > 0 { parts.append(agents == 1 ? "1 agent" : "\(agents) agents") }
        return parts.joined(separator: ", ")
    }

    /// The pills for a `Rail` (`RailModel.derive`, unchanged) and the live level bars. The agents pill carries every mark
    /// (gold needs-you dots first, then the agents) — `Rail` already caps them at `maxDots` with `overflow`.
    public static func derive(rail: Rail, levels: [Float], hovered: Kind?) -> RailPills {
        RailPills(pills: [
            Pill(kind: .inbox, count: rail.needsYouCount, hovered: hovered == .inbox),
            Pill(kind: .agents, count: rail.marks.count + rail.overflow, dots: Array(rail.marks.prefix(maxDots)),
                 overflow: rail.overflow + max(0, rail.marks.count - maxDots), hovered: hovered == .agents),
            Pill(kind: .actions, levels: levels.map { min(1, max(0, $0)) }, hovered: hovered == .actions),
            Pill(kind: .more, hovered: hovered == .more),
        ])
    }
}
