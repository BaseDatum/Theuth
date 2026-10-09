import Foundation

// 153-CC stub — C1 fills (§10.3 step-typed rows, §10.4 turn cap, §10.6 approval box).

/// The command card's reducer: desk-session events → step rows + the ✓/✗ verdict. Pure, tested on the Mac for both platforms.
public struct CommandCard: Equatable, Sendable {
    /// W-e: tool rounds per request; at the cap the controller sends `cancel(desk, root)`.
    public static let maxSteps = 12
    /// Row text cap (§10.3: `purpose` after the colon, ≤ 60 chars).
    public static let textCap = 60

    public struct Row: Equatable, Sendable, Identifiable {
        /// From the `purpose` prefix ("checking: …"); `result` when it has none.
        public enum Kind: String, Equatable, Sendable { case checking, executing, verifying, result }
        /// The tool call id.
        public var id: String
        public var kind: Kind
        public var text: String
        public var purpose: String?
        /// `nil` running (shimmer), `true` ok (green), `false` error (red).
        public var ok: Bool?

        public init(id: String, kind: Kind, text: String, purpose: String? = nil, ok: Bool? = nil) {
            self.id = id; self.kind = kind; self.text = text; self.purpose = purpose; self.ok = ok
        }
    }

    /// The desk-session events the card reads (the stream the companion already subscribes to).
    public enum Event: Equatable, Sendable {
        case toolCallStarted(id: String, tool: String, purpose: String?)
        case toolCallFinished(id: String, ok: Bool)
        case textDelta(String)
        case turnFinished(stopReason: String)
    }

    /// §10.3: the final assistant line — "✓ …" / "✗ …" — or shown as is with the status "replied".
    public enum Verdict: Equatable, Sendable {
        case ok(String)
        case failed(String)
        case replied(String)
    }

    public var request = ""
    public var rows: [Row] = []
    /// `ToolCallStarted`s counted toward `maxSteps`.
    public var steps = 0
    public var text = ""
    public var verdict: Verdict?
    /// "stopped after 12 steps — hold ⌃⌥C to continue" / "✗ couldn't finish — ‹message›".
    public var frozen: String?

    public init(request: String = "") { self.request = request }

    /// Reduce one event. Answers `true` when this event hit the cap (the controller cancels the turn).
    @discardableResult
    public mutating func observe(_ event: Event) -> Bool {
        // 153-CC stub — C1 fills.
        _ = event
        return false
    }

    /// `"✓ done"` → `.ok("done")`, `"✗ …"` → `.failed(…)`, else `.replied(text)`.
    public static func parseVerdict(_ text: String) -> Verdict {
        // 153-CC stub — C1 fills.
        .replied(text)
    }

    /// `"checking: ls …"` → (`.checking`, `"ls …"`); no known prefix → (`.result`, the text).
    public static func parsePurpose(_ purpose: String?, tool: String) -> (Row.Kind, String) {
        // 153-CC stub — C1 fills.
        (.result, purpose ?? tool)
    }
}
