import Foundation
import TheuthKit

// 153-CC stub — C1 fills (§10.6 grammar + §7.1 near-miss tolerance).

/// The spoken approval grammar: the first final utterance while the approval box is up → `Decision?`.
public enum SpokenApproval {
    /// §10.6: no match in this long closes the listen (the keys still work).
    public static let listenSecs = 8
    public static let allow = ["yes", "yeah", "go", "go ahead", "do it", "approve"]
    public static let deny = ["no", "nope", "stop", "cancel", "deny"]

    /// `nil` = the words are ignored and the box stays ("didn't catch that — ⏎ or ⎋"). Exact phrase first, then the
    /// §7.1 near-miss tolerance (`len / 5`, phrases ≥ 5 chars only).
    public static func decide(_ utterance: String) -> Decision? {
        // 153-CC stub — C1 fills.
        _ = utterance
        return nil
    }
}
