import Foundation
import TheuthKit

/// The one answer card's state (153 §12, D12/D13): what it shows, which generation it belongs to, when it may hide.
/// Pure, no AppKit; the Companion's `AnswerCard` panel renders it and `PointController` / the Ask bar (through
/// `Store.showAnswerCard`) drive it.
public struct AnswerCardState: Equatable, Sendable {
    /// §12.4 rows, in order of a gesture's life. "speaking" is the parallel `speaking` flag (a glyph, not a phase).
    public enum Phase: String, Equatable, Sendable { case listening, finishing, thinking, answer, failed }
    /// Which surface opened the card — the header label and the follow-up's marker family (§12.2).
    public enum Kind: String, Equatable, Sendable { case ask, voiceNote, command }

    /// One request in the thread (§12.2 session dots): the question, the streamed answer, the tool chips, the command rows.
    public struct Turn: Equatable, Sendable {
        public var question: String
        public var text = ""
        public var tools: [String] = []
        /// Set for `.command` turns: the step rows and the verdict (§10.3).
        public var command: CommandCard?
        public init(question: String, command: CommandCard? = nil) { self.question = question; self.command = command }
    }

    /// §10.6: the desk's pending approval, shown inside the card until answered.
    public struct Approval: Equatable, Sendable {
        public enum Outcome: Equatable, Sendable { case allowed, denied, timedOut }
        public var id: String
        public var tool: String
        /// The exact target (the args' command/path/script, clipped).
        public var target: String
        public var risk: String
        /// The 8 s spoken-approval window is open.
        public var listening = false
        /// The last utterance matched nothing ("didn't catch that — ⏎ or ⎋").
        public var missed = false
        public var outcome: Outcome?
        public init(id: String, tool: String, target: String, risk: String) {
            self.id = id; self.tool = tool; self.target = target; self.risk = risk
        }
    }

    /// §12.4: the answer hides this long after `TurnFinished` — only when collapsed and not speaking.
    public static let doneMs = 6_000

    public var phase: Phase = .listening
    public var kind: Kind = .voiceNote
    /// The desk session the thread lives in (O opens it; follow-ups go to it).
    public var session: String?
    public var turns: [Turn] = []
    /// The turn on view (J/K move it); reducers write the last one.
    public var current = 0
    /// The copy column of §12.4 ("finishing…", "thinking · code_refs · 4 s", "couldn't reply — ‹why›").
    public var status = ""
    /// `thinking` entered at this wall time: the view ticks the elapsed seconds so the card never looks frozen.
    public var thinkingSince: Date?
    public var expanded = false
    public var speaking = false
    public var turnFinished = false
    public var visible = false
    public var approval: Approval?
    /// Bumped by every `begin`/`followUp`; a `hide(generation:)` older than it is ignored (the 149f rule).
    public private(set) var generation = 0

    public init() {}

    public var question: String { turns.last?.question ?? "" }
    public var text: String {
        get { turns.last?.text ?? "" }
        set { if !turns.isEmpty { turns[turns.count - 1].text = newValue } }
    }
    public var tools: [String] {
        get { turns.last?.tools ?? [] }
        set { if !turns.isEmpty { turns[turns.count - 1].tools = newValue } }
    }
    public var command: CommandCard? {
        get { turns.last?.command }
        set { if !turns.isEmpty { turns[turns.count - 1].command = newValue } }
    }
    public var failed: Bool { phase == .failed }
    /// The marker the follow-up field prepends (§12.2): none for Ask, `[voice note, N s]` / `[command, N s]` otherwise.
    public static func marker(_ kind: Kind, durationMs: UInt64) -> String {
        let secs = max(1, (durationMs + 999) / 1000)
        switch kind {
        case .ask: return ""
        case .voiceNote: return "[voice note, \(secs) s] "
        case .command: return "[command, \(secs) s] "
        }
    }

    /// A new gesture: everything resets, the generation moves on. 189 F5: an Ask opens expanded, so its answer streams as
    /// markdown from the first delta (an expanded card never auto-hides: `armsDone`); voice and command cards collapsed.
    public mutating func begin(question: String, kind: Kind = .voiceNote) {
        var fresh = Self()
        fresh.generation = generation + 1
        fresh.kind = kind
        fresh.turns = [Turn(question: question, command: kind == .command ? CommandCard(request: question) : nil)]
        fresh.visible = true
        fresh.expanded = kind == .ask
        fresh.phase = .thinking
        fresh.thinkingSince = Date()
        self = fresh
    }

    /// 189 F7: what the markdown body renders for turn `i` — the last turn's text with `MarkdownLines.holdBack` while it
    /// is unfinished, every finished turn whole.
    public func markdownText(turn i: Int) -> String {
        guard turns.indices.contains(i) else { return "" }
        return i == turns.count - 1 && !turnFinished ? MarkdownLines.holdBack(turns[i].text) : turns[i].text
    }

    /// §10.5 / §12.2: a follow-up in the same thread — a new block under the previous answer, the dots advance.
    public mutating func followUp(question: String) {
        generation += 1
        turns.append(Turn(question: question, command: kind == .command ? CommandCard(request: question) : nil))
        current = turns.count - 1
        phase = .thinking
        thinkingSince = Date()
        status = ""
        turnFinished = false
        approval = nil
        visible = true
    }

    /// Hide only when `generation` is the latest; a stale timer's hide is ignored. Answers whether it applied.
    @discardableResult
    public mutating func hide(generation g: Int) -> Bool {
        guard g == generation else { return false }
        visible = false
        return true
    }

    /// §12.4 arming rule: `doneMs` runs only after `TurnFinished` *and* speech ended *and* collapsed; never in `failed`,
    /// never while an approval waits.
    public var armsDone: Bool {
        phase == .answer && turnFinished && !speaking && !expanded && (approval == nil || approval?.outcome != nil)
    }

    /// J/K: move the viewed turn; clamped.
    public mutating func move(_ delta: Int) {
        current = min(max(0, current + delta), max(0, turns.count - 1))
    }
}
