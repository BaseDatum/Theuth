import Foundation
import TheuthKit

// 149 W1 fills the bodies (design 149 §3.3, Revision 1 rows 2, 3, 6, 7). C fixes the shapes; `Card.make` is unchanged.

/// The card's visible parts in F4/F5 order: header · status line · bubbles · options · footer.
public struct CardAnatomy: Equatable, Sendable {
    public struct Header: Equatable, Sendable {
        public var name: String
        /// 1-based position and the count behind the dots (VoiceOver "card 2 of 3").
        public var position: Int
        public var count: Int
        /// The glance sentence under the header (Revision 1 row 7).
        public var glance: String
        public init(name: String, position: Int, count: Int, glance: String) {
            self.name = name; self.position = position; self.count = count; self.glance = glance
        }
    }

    /// `‹state› · ‹relative time› · ‹project›` with the harness as the icon (Revision 1 row 6).
    public struct StatusLine: Equatable, Sendable {
        public var harness: String
        public var state: String
        public var age: String
        public var project: String?
        public init(harness: String, state: String, age: String, project: String? = nil) {
            self.harness = harness; self.state = state; self.age = age; self.project = project
        }
    }

    /// The conversation pair: E6's `ItemContext` or the interim join (`title`/`detail`).
    public struct Bubble: Equatable, Sendable {
        public enum Side: Equatable, Sendable { case user, agent }
        public var side: Side
        public var text: String
        public init(side: Side, text: String) { self.side = side; self.text = text }
    }

    public enum FooterVerb: Equatable, Sendable, CaseIterable {
        case reply, point, talk
        /// Only when the agent runs or waits (Revision 1 row 2).
        case stop
        /// Only when `clearable` (Revision 1 row 3).
        case clear
        case snooze, legend
    }

    public var header: Header
    public var status: StatusLine
    public var bubbles: [Bubble]
    public var options: [CardOption]
    public var footer: [FooterVerb]
    public var ifWrong: String?

    public init(header: Header, status: StatusLine, bubbles: [Bubble], options: [CardOption], footer: [FooterVerb], ifWrong: String?) {
        self.header = header; self.status = status; self.bubbles = bubbles; self.options = options; self.footer = footer; self.ifWrong = ifWrong
    }

    /// "just now" under 45 s, then `RailModel.ago`.
    public static let justNow: TimeInterval = 45

    /// Bubble texts are capped here too (E6 caps at 160; the interim pair is not).
    public static let bubbleChars = 160

    public static func make(card: Card, position: Int, count: Int, glance: String, activity: AgentInfo?, session: SessionSummary?,
                            now: Date) -> CardAnatomy {
        let item = card.item
        let age = now.timeIntervalSince(item.waitingSince) < justNow ? "just now" : RailModel.ago(item.waitingSince, now)
        // Revision 1 row 6: project first (`AgentInfo.project`, then `cwd`'s last component), the harness as the icon,
        // the harness key as text only when no project is known (§12.1 item 6).
        let harnessRaw = activity?.agent ?? session?.agent
        let cwdLeaf = session?.cwd.flatMap { c -> String? in
            let leaf = c.split(separator: "/").last.map(String.init)
            return leaf?.isEmpty == false ? leaf : nil
        }
        let project = activity?.project.flatMap { $0.isEmpty ? nil : $0 } ?? session?.project.flatMap { $0.isEmpty ? nil : $0 } ?? cwdLeaf ?? harnessRaw
        let status = StatusLine(harness: AgentsModel.harnessKey(harnessRaw), state: state(item.kind, activity: activity), age: age, project: project)

        // D4: E6's pair when present, else the interim `SessionSummary.title` + `AgentInfo.last`; never both halves missing.
        var bubbles: [Bubble] = []
        if let c = item.context {
            if let r = clip(c.request) { bubbles.append(Bubble(side: .user, text: r)) }
            if let l = clip(c.lastReply) { bubbles.append(Bubble(side: .agent, text: l)) }
        } else {
            if let t = clip(session?.title) { bubbles.append(Bubble(side: .user, text: t)) }
            if let l = clip(activity?.last) { bubbles.append(Bubble(side: .agent, text: l)) }
        }

        var footer: [FooterVerb] = []
        if card.replyAction("") != nil { footer += [.reply, .talk] }
        if item.session != nil { footer.append(.point) }
        // D5 / Revision 1 row 2: Stop only on a needs-you card whose agent runs or waits; a Finished item is a clean end_turn.
        if item.session != nil, Self.stoppableKinds.contains(item.kind), let a = activity, isRunningOrWaiting(a.state) { footer.append(.stop) }
        if card.clearable { footer.append(.clear) }
        if item.kind.needsYou { footer.append(.snooze) }
        footer.append(.legend)

        return CardAnatomy(header: Header(name: card.who, position: position, count: count, glance: glance), status: status,
                           bubbles: bubbles, options: card.options, footer: footer, ifWrong: card.ifWrong)
    }

    /// Revision 1 row 2: ask · approval · blocked · decision (· review in flight) — never finished.
    public static let stoppableKinds: Set<AttentionKind> = [.ask, .approval, .blocked, .decision, .review]

    static func isRunningOrWaiting(_ s: AgentState) -> Bool {
        switch s {
        case .running, .awaiting, .approval: return true
        case .idle, .error, .cancelled: return false
        }
    }

    private static func clip(_ s: String?) -> String? {
        guard let s = s?.trimmingCharacters(in: .whitespacesAndNewlines), !s.isEmpty else { return nil }
        return s.count > bubbleChars ? String(s.prefix(bubbleChars - 1)) + "…" : s
    }

    /// `‹state›` (Revision 1 row 6): the kind's wait, "Stopped" after a cancel (§5.4 B).
    private static func state(_ kind: AttentionKind, activity: AgentInfo?) -> String {
        if activity?.state == .cancelled { return "Stopped" }
        switch kind {
        case .finished: return "Finished"
        case .ask: return "Waiting for your answer"
        case .approval: return "Waiting for approval"
        case .decision: return "Waiting for a decision"
        case .review: return "Waiting for review"
        case .blocked: return "Blocked"
        case .hire: return "Proposed"
        case .budget: return "Over budget"
        case .resume: return "Ready to resume"
        case .show: return "Shown"
        case .node, .handoff, .secret, .other: return "Needs you"
        }
    }
}
