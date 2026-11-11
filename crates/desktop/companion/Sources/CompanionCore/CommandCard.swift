import Foundation

/// The command card's reducer (153 §10.3 step-typed rows, §10.4 turn cap, §10.6 approval row): desk-session events →
/// step rows + the ✓/✗ verdict. Pure, tested on the Mac for both platforms.
public struct CommandCard: Equatable, Sendable {
    /// W-e: tool rounds per request; at the cap the controller sends `cancel(desk, root)`.
    public static let maxSteps = 12
    /// Row text cap (§10.3: `purpose` after the colon, ≤ 60 chars).
    public static let textCap = 60
    public static let stoppedCopy = "stopped after \(maxSteps) steps — hold ⌃⌥C to continue"

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

        /// §10.3's first column: "checked · executed · verifying · next" — past tense once returned, present while running,
        /// `next` for a call started behind the running one.
        public func label(queued: Bool) -> String {
            if queued { return "next" }
            switch (kind, ok) {
            case (.checking, nil): return "checking"
            case (.checking, _): return "checked"
            case (.executing, nil): return "executing"
            case (.executing, _): return "executed"
            case (.verifying, nil): return "verifying"
            case (.verifying, _): return "verified"
            case (.result, nil): return "running"
            case (.result, _): return "ran"
            }
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

        public var text: String {
            switch self { case .ok(let t), .failed(let t), .replied(let t): return t }
        }
    }

    public var request = ""
    public var rows: [Row] = []
    /// `ToolCallStarted`s counted toward `maxSteps`.
    public var steps = 0
    public var text = ""
    public var verdict: Verdict?
    /// "stopped after 12 steps — hold ⌃⌥C to continue" / "✗ couldn't finish — ‹message›".
    public var frozen: String?
    /// The tool names in start order (the §12.2 chips and the done line's "ls · mkdir/mv · ls").
    public var tools: [String] = []

    public init(request: String = "") { self.request = request }

    public var finished: Bool { verdict != nil || frozen != nil }

    /// Reduce one event. Answers `true` when this event hit the cap (the controller cancels the turn).
    @discardableResult
    public mutating func observe(_ event: Event) -> Bool {
        switch event {
        case .toolCallStarted(let id, let tool, let purpose):
            guard frozen == nil, verdict == nil else { return false }
            let (kind, text) = Self.parsePurpose(purpose, tool: tool)
            rows.append(Row(id: id, kind: kind, text: text, purpose: purpose))
            tools.append(tool)
            steps += 1
            guard steps >= Self.maxSteps else { return false }
            frozen = Self.stoppedCopy
            return true
        case .toolCallFinished(let id, let ok):
            if let i = rows.lastIndex(where: { $0.id == id }) { rows[i].ok = ok }
        case .textDelta(let t):
            guard frozen == nil else { return false }
            text += t
        case .turnFinished(let stopReason):
            finish(text: text, failure: AskReply.terminalFailure(stopReason))
        }
        return false
    }

    /// The reply finished (189 D2/D3): `text` is the authoritative answer (the transcript's), a failure freezes the card
    /// with its reason. A card already frozen or decided stays.
    public mutating func finish(text: String, failure: String?) {
        guard frozen == nil, verdict == nil else { return }
        self.text = text
        if let failure { frozen = "✗ couldn't finish — \(failure)" } else { verdict = Self.parseVerdict(text) }
    }

    /// `"✓ done"` → `.ok("done")`, `"✗ …"` → `.failed(…)`, else `.replied(text)`.
    public static func parseVerdict(_ text: String) -> Verdict {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let first = t.first else { return .replied(t) }
        let rest = t.dropFirst().trimmingCharacters(in: .whitespaces)
        switch first {
        case "✓": return .ok(rest)
        case "✗": return .failed(rest)
        default: return .replied(t)
        }
    }

    /// `"checking: ls …"` → (`.checking`, `"ls …"`); no known prefix → (`.result`, the text); text ≤ `textCap`.
    public static func parsePurpose(_ purpose: String?, tool: String) -> (Row.Kind, String) {
        guard let p = purpose?.trimmingCharacters(in: .whitespacesAndNewlines), !p.isEmpty else { return (.result, clip(tool)) }
        for kind in [Row.Kind.checking, .executing, .verifying] where p.lowercased().hasPrefix(kind.rawValue + ":") {
            let body = p.dropFirst(kind.rawValue.count + 1).trimmingCharacters(in: .whitespaces)
            return (kind, clip(body.isEmpty ? tool : body))
        }
        return (.result, clip(p))
    }

    static func clip(_ s: String) -> String {
        let line = s.split(whereSeparator: \.isNewline).first.map(String.init) ?? s
        return line.count > textCap ? String(line.prefix(textCap - 1)) + "…" : line
    }
}
