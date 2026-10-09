import Foundation

// 153-CC stub — C1 fills (§12.4 state table, §12.3 expand/collapse). Pure, no AppKit; the Companion's
// `AnswerCardState` view-model (PointAndSay/AnswerCard.swift) shadows this name inside that module — qualify as
// `CompanionCore.AnswerCardState` there until C1 merges the two.

/// The one answer card's state (D12/D13): what it shows, which generation it belongs to, when it may hide.
public struct AnswerCardState: Equatable, Sendable {
    /// §12.4 rows, in order of a gesture's life.
    public enum Phase: String, Equatable, Sendable { case listening, finishing, thinking, answer, speaking, failed }

    /// §12.4: the answer hides this long after `TurnFinished` — only when collapsed and not speaking.
    public static let doneMs = 6_000

    public var phase: Phase = .listening
    public var question = ""
    /// The streamed answer (markdown).
    public var text = ""
    /// The copy column of §12.4 ("finishing…", "thinking · code_refs · 4 s", "couldn't reply — ‹why›").
    public var status = ""
    /// Tool chips as they start (thinking).
    public var tools: [String] = []
    public var expanded = false
    public var speaking = false
    public var turnFinished = false
    public var visible = false
    /// Bumped by every `begin`; a `hide(generation:)` older than it is ignored (the 149f rule).
    public private(set) var generation = 0

    public init() {}

    /// A new gesture: everything resets, the generation moves on.
    public mutating func begin(question: String) {
        var fresh = Self()
        fresh.generation = generation + 1
        fresh.question = question
        fresh.visible = true
        self = fresh
    }

    /// Hide only when `generation` is the latest; a stale timer's hide is ignored. Answers whether it applied.
    @discardableResult
    public mutating func hide(generation g: Int) -> Bool {
        guard g == generation else { return false }
        visible = false
        return true
    }

    /// §12.4 arming rule: `doneMs` runs only after `TurnFinished` *and* speech ended *and* collapsed; never in `failed`.
    public var armsDone: Bool { phase == .answer && turnFinished && !speaking && !expanded }
}
