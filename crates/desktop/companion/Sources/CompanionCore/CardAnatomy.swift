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

    public static func make(card: Card, position: Int, count: Int, glance: String, activity: AgentInfo?, session: SessionSummary?,
                            now: Date) -> CardAnatomy {
        fatalError("149 W1")
    }
}
