import Foundation

/// One read of the target field (153 §13.4): `kAXValue` (≤ 60 000 chars), `kAXNumberOfCharacters`, `kAXSelectedTextRange.location`.
/// Any of them `nil` when the element does not report it; Windows: `TextPattern.DocumentRange.GetText(60000)` + `GetSelection`.
public struct FieldRead: Equatable, Sendable {
    public var value: String?
    public var count: Int?
    public var caret: Int?
    public init(value: String? = nil, count: Int? = nil, caret: Int? = nil) {
        self.value = value; self.count = count; self.caret = caret
    }
}

/// §13.4: three reads after the paste → did our text land? Pure: the caller feeds the reads (`observe`) or hands a
/// read callback + an injectable sleep to `run`. Direct insertions verify once (`reads: 1`, their 150 ms read-back).
public struct PasteVerifier: Equatable, Sendable {
    /// Read at these delays after the paste keystroke.
    public static let readDelaysMs = [150, 350, 1000]
    /// `kAXValue` is read at most this long.
    public static let maxValueChars = 60_000

    public enum Outcome: Equatable, Sendable {
        /// A change containing our text.
        case landed
        /// No card: the focus moved (`FieldRead` nil — a different element or none) or the user typed / moved the caret.
        case unknown(Reason)
        /// Every read identical to the one before the paste → the failure card (§13.5).
        case notLanded
    }
    public enum Reason: Equatable, Sendable { case focusMoved, userActed }

    public let text: String
    public let before: FieldRead
    /// How many reads decide `notLanded` (3 for the lease, 1 for a direct insertion).
    public let reads: Int
    public private(set) var seen = 0
    public private(set) var outcome: Outcome?

    public init(text: String, before: FieldRead, reads: Int = 3) {
        self.text = text; self.before = before; self.reads = max(1, reads)
    }

    /// Feed the next read (`nil` = the focused element is not the captured one any more). Answers the outcome once decided;
    /// further reads are ignored.
    @discardableResult
    public mutating func observe(_ read: FieldRead?) -> Outcome? {
        if let outcome { return outcome }
        seen += 1
        guard let read else { return decide(.unknown(.focusMoved)) }
        if read == before { return seen >= reads ? decide(.notLanded) : nil }
        return decide(Self.containsMore(of: text, in: read, than: before) ? .landed : .unknown(.userActed))
    }

    /// One shot over already-collected reads.
    public static func verify(text: String, before: FieldRead, reads: [FieldRead?]) -> Outcome {
        var v = PasteVerifier(text: text, before: before, reads: reads.count)
        for r in reads { if let o = v.observe(r) { return o } }
        return v.outcome ?? .notLanded
    }

    /// The timed triple: `sleep(ms)` then `read()` per delay, stopping at the first decision.
    public static func run(text: String, before: FieldRead, delaysMs: [Int] = readDelaysMs,
                           sleep: (Int) async -> Void, read: () async -> FieldRead?) async -> Outcome {
        var v = PasteVerifier(text: text, before: before, reads: delaysMs.count)
        var elapsed = 0
        for at in delaysMs {
            await sleep(max(0, at - elapsed)); elapsed = at
            if let o = v.observe(await read()) { return o }
        }
        return v.outcome ?? .notLanded
    }

    /// "A change containing our text": the value holds one more occurrence of `text` than before; without a readable
    /// value, the character count grew.
    static func containsMore(of text: String, in read: FieldRead, than before: FieldRead) -> Bool {
        if let v = read.value {
            guard !text.isEmpty else { return false }
            return occurrences(of: text, in: v) > occurrences(of: text, in: before.value ?? "")
        }
        if let n = read.count, let m = before.count { return n > m }
        return false
    }

    static func occurrences(of needle: String, in hay: String) -> Int {
        var n = 0, from = hay.startIndex
        while let r = hay.range(of: needle, range: from..<hay.endIndex) { n += 1; from = r.upperBound }
        return n
    }

    private mutating func decide(_ o: Outcome) -> Outcome { outcome = o; return o }
}
