import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W1: the four pills from a `Rail` fixture (design 149 §3.1, §10).
struct RailPillsModelTests {
    private func mark(_ id: String, _ state: MarkState) -> Mark {
        Mark(id: id, state: state, label: id, detail: id, since: nil, itemId: nil, session: "s-\(id)")
    }

    private func rail(needs: Int, agents: Int, overflow: Int = 0) -> Rail {
        Rail(needsYou: (0..<needs).map { mark("n\($0)", .needsYou(.ask)) },
             agents: (0..<agents).map { mark("a\($0)", $0 % 2 == 0 ? .working : .idle) },
             overflow: overflow, needsYouCount: needs, workingCount: (agents + 1) / 2)
    }

    @Test func inboxPillCountsNeedsYouAndGoesGoldOnlyAboveZero() {
        let quiet = RailPills.derive(rail: .empty, levels: [], hovered: nil)
        #expect(quiet[.inbox].count == 0, "no count, no gold")
        #expect(quiet.pills.map(\.kind) == [.inbox, .agents, .actions, .more], "the pill order is the stack order")
        let busy = RailPills.derive(rail: rail(needs: 3, agents: 2), levels: [], hovered: nil)
        #expect(busy[.inbox].count == 3)
        #expect(busy[.inbox].dots.isEmpty, "the count rides on the inbox pill; the dots live on the agents pill")
    }

    @Test func agentsPillShowsAtMostTwelveDotsThenPlusN() {
        let p = RailPills.derive(rail: rail(needs: 3, agents: 9), levels: [], hovered: nil)
        #expect(p[.agents].dots.count == 12)
        #expect(p[.agents].overflow == 0)
        #expect(p[.agents].dots.prefix(3).allSatisfy { $0.isNeedsYou }, "gold needs-you dots first")
        // `Rail` already capped at 12 with `overflow` 4 (RailModel.derive); the pill passes it through.
        let capped = RailPills.derive(rail: rail(needs: 3, agents: 9, overflow: 4), levels: [], hovered: nil)
        #expect(capped[.agents].overflow == 4)
        #expect(capped[.agents].count == 16, "the pill knows the whole population")
        // A hand-built rail over the cap is trimmed too.
        let over = RailPills.derive(rail: rail(needs: 0, agents: 15), levels: [], hovered: nil)
        #expect(over[.agents].dots.count == 12)
        #expect(over[.agents].overflow == 3)
        #expect(RailPills.derive(rail: .empty, levels: [], hovered: nil)[.agents].dots.isEmpty, "no agents: the pill stays, empty")
    }

    @Test func actionsPillDrawsLevelBarsFromLevels() {
        let p = RailPills.derive(rail: .empty, levels: [0.2, 0.6, 1.4, -0.1], hovered: nil)
        #expect(p[.actions].levels == [0.2, 0.6, 1, 0], "one bar per band, clamped 0–1, order kept (newest right)")
        #expect(p[.inbox].levels.isEmpty && p[.agents].levels.isEmpty && p[.more].levels.isEmpty)
        #expect(RailPills.derive(rail: .empty, levels: [], hovered: nil)[.actions].levels.isEmpty, "quiet mic: the glyph, no bars")
    }

    @Test func hoverGrowsOnlyTheHoveredPill() {
        let p = RailPills.derive(rail: rail(needs: 1, agents: 1), levels: [], hovered: .agents)
        #expect(p.pills.filter(\.hovered).map(\.kind) == [.agents])
        #expect(RailPills.derive(rail: .empty, levels: [], hovered: nil).pills.allSatisfy { !$0.hovered })
    }

    @Test func collapsedSliverKeepsTheCounts() {
        #expect(RailPills.derive(rail: .empty, levels: [], hovered: nil).sliverLabel == "Theuth rail, nothing needs you")
        #expect(RailPills.derive(rail: rail(needs: 1, agents: 0), levels: [], hovered: nil).sliverLabel == "Theuth rail, 1 needs you")
        let p = RailPills.derive(rail: rail(needs: 3, agents: 9, overflow: 4), levels: [], hovered: nil)
        #expect(p.sliverLabel == "Theuth rail, 3 need you, 13 agents", "the overflow is counted, not hidden; needs-you dots are not agents")
        #expect(RailPills.stackHeight == 4 * 52 + 3 * 12, "D1 geometry: 52 pt pills, 12 pt gaps")
    }
}
