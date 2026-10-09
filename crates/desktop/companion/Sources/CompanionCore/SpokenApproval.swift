import Foundation
import TheuthKit

/// The spoken approval grammar (153 §10.6): the first final utterance while the approval box is up → `Decision?`.
public enum SpokenApproval {
    /// §10.6: no match in this long closes the listen (the keys still work).
    public static let listenSecs = 8
    public static let allow = ["yes", "yeah", "go", "go ahead", "do it", "approve"]
    public static let deny = ["no", "nope", "stop", "cancel", "deny"]
    public static let missedCopy = "didn't catch that — ⏎ or ⎋"

    /// `nil` = the words are ignored and the box stays ("didn't catch that — ⏎ or ⎋"). Exact phrase first, then the
    /// §7.1 near-miss tolerance (`len / 5` edits, phrases ≥ 5 chars only); a tie between the two lists is no answer.
    public static func decide(_ utterance: String) -> Decision? {
        let heard = normalise(utterance)
        guard !heard.isEmpty else { return nil }
        if allow.contains(heard) { return .allow }
        if deny.contains(heard) { return .deny }
        let a = nearest(heard, in: allow), d = nearest(heard, in: deny)
        switch (a, d) {
        case (nil, nil): return nil
        case (.some, nil): return .allow
        case (nil, .some): return .deny
        case (.some(let x), .some(let y)): return x == y ? nil : (x < y ? .allow : .deny)
        }
    }

    /// Lowercase, punctuation out, one space between words.
    static func normalise(_ s: String) -> String {
        s.lowercased().unicodeScalars
            .map { CharacterSet.alphanumerics.contains($0) ? Character($0) : " " }
            .reduce(into: "") { $0.append($1) }
            .split(separator: " ").joined(separator: " ")
    }

    /// The smallest edit distance to a phrase within its tolerance, or nil.
    private static func nearest(_ heard: String, in phrases: [String]) -> Int? {
        phrases.compactMap { p -> Int? in
            guard p.count >= 5 else { return nil }
            let d = levenshtein(heard, p)
            return d <= p.count / 5 ? d : nil
        }.min()
    }

    static func levenshtein(_ a: String, _ b: String) -> Int {
        let a = Array(a), b = Array(b)
        if a.isEmpty { return b.count }
        if b.isEmpty { return a.count }
        var prev = Array(0...b.count), cur = [Int](repeating: 0, count: b.count + 1)
        for i in 1...a.count {
            cur[0] = i
            for j in 1...b.count {
                cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (a[i - 1] == b[j - 1] ? 0 : 1))
            }
            swap(&prev, &cur)
        }
        return prev[b.count]
    }
}
