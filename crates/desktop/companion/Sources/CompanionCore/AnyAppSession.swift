import Foundation
import TheuthKit

/// 153 §3.2 / §13: one ⌃⌥D gesture's decisions, pure. The controller reads AX into `AXSnapshot`, resolves the `AppProfile`
/// and asks this value what to do: refuse at the press, dictate (direct insertion or the lease per utterance), or rewrite
/// the selection; whether "send it" may press Return; what "scratch that" may do. D1's machines decide the verdict, the
/// lease and the verification; this one holds the gesture's memory of them. The Windows twin mirrors it 1:1.
public struct AnyAppSession: Equatable, Sendable {
    /// §13.2: direct `kAXSelectedText` write, else §13.3's clipboard lease + ⌘V.
    public enum Method: Equatable, Sendable { case direct, lease }

    /// What the press opens.
    public enum Entry: Equatable, Sendable {
        /// §13.1 certainly-not / §13.8: no dictation; the pill shows `hint` for 2.5 s.
        case refused(hint: String)
        /// Words are delivered as utterances close; `label` is the pill's copy ("Dictating into Mail").
        case dictate(label: String)
        /// §13.6: a non-empty selection — the spoken words are the instruction.
        case rewrite(chars: Int)
    }

    /// One closed utterance that went to the target.
    public struct Delivered: Equatable, Sendable {
        public var utterance: Int
        /// The payload as written (spacing included).
        public var text: String
        public var method: Method
        /// Read-back / verification said `landed`.
        public var verified: Bool
        public init(utterance: Int, text: String, method: Method, verified: Bool) {
            self.utterance = utterance; self.text = text; self.method = method; self.verified = verified
        }
    }

    /// §7.4 "scratch that" on the last delivery.
    public enum Scratch: Equatable, Sendable {
        /// Editable, delivered directly: select back `chars` through the AX range and replace with nothing.
        case selectBack(chars: Int)
        /// Pasted and verified: ⌫ × `chars`.
        case backspace(chars: Int)
        /// Not ours, not verified, or not the last line: nothing happens, the pill says why.
        case ignore(hint: String)
    }

    /// The refusal copy (§3.2 state table / §13.8).
    public static let refusalPrefix = "Nothing to type into — "
    public static let ownWindowHint = "Use Ctrl+D here"
    public static let lockedHint = "Screen locked"
    public static let blindHint = "Accessibility off — pasting blind"
    public static let scratchHint = "Nothing of ours to scratch"
    /// §13.8: more than this in one gesture goes to the card, not the app.
    public static let maxCharsPerGesture = 20_000

    public let snapshot: AXSnapshot
    public let profile: AppProfile
    public let verdict: DeliveryAssessment.Verdict
    /// `AXIsProcessTrusted()` at the press: without it the assessment is `unknown` and the pill says so.
    public let trusted: Bool
    public let selection: String
    /// Set when the focused element is one of ours (the companion, theuth.app) or the screen is locked.
    public let refusal: String?
    /// §13.2: one failed read-back → the lease for every later utterance of the gesture.
    public private(set) var directFailed = false
    public private(set) var delivered: [Delivered] = []
    /// §13.6: earlier (instruction, result) pairs for `rewrite(thread:)`.
    public private(set) var thread: [[String]] = []

    public init(snapshot: AXSnapshot, profile: AppProfile, selection: String? = nil, trusted: Bool = true, ownWindow: Bool = false,
                screenLocked: Bool = false) {
        self.snapshot = snapshot; self.profile = profile; self.trusted = trusted
        self.selection = selection ?? ""
        verdict = DeliveryAssessment.assess(snapshot, terminal: profile.terminal)
        refusal = screenLocked ? Self.lockedHint : ownWindow ? Self.ownWindowHint : nil
    }

    public var entry: Entry {
        if let refusal { return .refused(hint: refusal) }
        if case .certainlyNot(let r) = verdict { return .refused(hint: Self.refusalPrefix + DeliveryAssessment.label(r)) }
        if !selection.isEmpty { return .rewrite(chars: selection.count) }
        return .dictate(label: label)
    }

    /// The pill's label after the chord: the app and how the words get there.
    public var label: String {
        if !trusted { return Self.blindHint }
        switch verdict {
        case .editable: return "Dictating into \(profile.name)"
        case .unknown: return "\(profile.name) · unknown field — paste"
        case .certainlyNot: return Self.refusalPrefix
        }
    }

    /// §13.2: direct only while the verdict is `editable` and no read-back failed.
    public var method: Method { verdict == .editable && !directFailed ? .direct : .lease }

    /// What goes into the field for utterance `i`: a space joins it to a previous delivery of ours unless the text already
    /// starts with whitespace or closing punctuation.
    public func payload(_ text: String) -> String {
        guard !delivered.isEmpty, let first = text.first, first.isLetter || first.isNumber || first == "“" || first == "(" else { return text }
        return " " + text
    }

    /// The §13.8 cap over everything delivered plus `text`.
    public func exceedsCap(adding text: String) -> Bool {
        delivered.reduce(text.count) { $0 + $1.text.count } > Self.maxCharsPerGesture
    }

    public mutating func directFailedOnce() { directFailed = true }

    public mutating func record(utterance: Int, text: String, method: Method, verified: Bool) {
        delivered.append(Delivered(utterance: utterance, text: text, method: method, verified: verified))
    }

    public func isDelivered(_ utterance: Int) -> Bool { delivered.contains { $0.utterance == utterance } }

    /// §7.4 / §13.8: Return only into `editable` / `unknown` targets that are not secure.
    public var returnAllowed: Bool {
        if case .certainlyNot = verdict { return false }
        return DeliveryAssessment.assess(snapshot, payload: .returnKey, terminal: profile.terminal) != .certainlyNot(.secure)
    }

    /// "scratch that" for utterance `i`: only the last delivery of ours can go.
    public mutating func scratch(_ i: Int) -> Scratch {
        guard let last = delivered.last, last.utterance == i else { return .ignore(hint: Self.scratchHint) }
        switch last.method {
        case .direct:
            delivered.removeLast()
            return .selectBack(chars: last.text.count)
        case .lease:
            guard last.verified else { return .ignore(hint: Self.scratchHint) }
            delivered.removeLast()
            return .backspace(chars: last.text.count)
        }
    }

    /// §13.6: the proposal for `instruction` came back as `result`; the next hold refines with the pair in the thread.
    public mutating func refined(instruction: String, result: String) { thread.append([instruction, result]) }

    /// "✓ 3 lines into Mail" (§3.2 done row).
    public var doneCopy: String {
        let n = delivered.count
        return "✓ \(n) \(n == 1 ? "line" : "lines") into \(profile.name)"
    }
}
