import Testing
import Foundation
import TheuthKit
@testable import CompanionCore

/// 153-D2: the gesture's decisions over fed snapshots — refuse path, direct vs lease, the Return rule, scratch-that, the
/// rewrite thread. D1's machines are tested in `AnyAppTests`.
struct AnyAppSessionTests {
    let mail = AppProfile(id: "com.apple.mail", name: "Mail")
    let terminal = AppProfile(id: "com.apple.Terminal", name: "Terminal", terminal: true)
    func field(_ role: String = "AXTextArea", secure: Bool = false) -> AXSnapshot { AXSnapshot(role: role, isEditable: true, isSecure: secure) }

    @Test func refusePath() {
        #expect(AnyAppSession(snapshot: AXSnapshot(role: "AXButton"), profile: mail).entry == .refused(hint: "Nothing to type into — button"))
        #expect(AnyAppSession(snapshot: AXSnapshot(role: "AXMenuItem"), profile: mail).entry == .refused(hint: "Nothing to type into — menu item"))
        #expect(AnyAppSession(snapshot: field(), profile: mail, ownWindow: true).entry == .refused(hint: AnyAppSession.ownWindowHint))
        #expect(AnyAppSession(snapshot: field(), profile: mail, screenLocked: true).entry == .refused(hint: AnyAppSession.lockedHint))
        // A selection on a refused target is still a refusal; a selection on a field is a rewrite.
        #expect(AnyAppSession(snapshot: AXSnapshot(role: "AXStaticText"), profile: mail, selection: "abc").entry == .refused(hint: "Nothing to type into — static text"))
        #expect(AnyAppSession(snapshot: field(), profile: mail, selection: "twelve chars").entry == .rewrite(chars: 12))
    }

    @Test func directVersusLease() {
        var s = AnyAppSession(snapshot: field(), profile: mail)
        #expect(s.entry == .dictate(label: "Dictating into Mail"))
        #expect(s.method == .direct)
        s.directFailedOnce()
        #expect(s.method == .lease, "one failed read-back → the lease for the rest of the gesture")
        let t = AnyAppSession(snapshot: field(), profile: terminal)
        #expect(t.verdict == .unknown)
        #expect(t.method == .lease)
        #expect(t.entry == .dictate(label: "Terminal · unknown field — paste"))
        let blind = AnyAppSession(snapshot: AXSnapshot(), profile: mail, trusted: false)
        #expect(blind.method == .lease)
        #expect(blind.entry == .dictate(label: AnyAppSession.blindHint))
    }

    @Test func returnRule() {
        #expect(AnyAppSession(snapshot: field(), profile: mail).returnAllowed)
        #expect(AnyAppSession(snapshot: AXSnapshot(role: "AXWebArea"), profile: mail).returnAllowed, "unknown targets take Return")
        #expect(!AnyAppSession(snapshot: field("AXSecureTextField"), profile: mail).returnAllowed, "never into a secure field")
        #expect(!AnyAppSession(snapshot: field(secure: true), profile: mail).returnAllowed)
        #expect(!AnyAppSession(snapshot: AXSnapshot(role: "AXButton"), profile: mail).returnAllowed)
    }

    @Test func scratchThatRule() {
        var s = AnyAppSession(snapshot: field(), profile: mail)
        #expect(s.scratch(0) == .ignore(hint: AnyAppSession.scratchHint), "nothing delivered yet")
        s.record(utterance: 0, text: "hello", method: .direct, verified: true)
        s.record(utterance: 1, text: " world", method: .direct, verified: true)
        #expect(s.scratch(0) == .ignore(hint: AnyAppSession.scratchHint), "only the last line")
        #expect(s.scratch(1) == .selectBack(chars: 6))
        #expect(s.delivered.count == 1)
        s.record(utterance: 2, text: " pasted", method: .lease, verified: false)
        #expect(s.scratch(2) == .ignore(hint: AnyAppSession.scratchHint), "a paste we could not verify is not ours to delete")
        #expect(s.delivered.count == 2, "ignored → stays recorded")
        var v = AnyAppSession(snapshot: AXSnapshot(role: "AXWebArea"), profile: mail)
        v.record(utterance: 0, text: "git status", method: .lease, verified: true)
        #expect(v.scratch(0) == .backspace(chars: 10))
        #expect(v.delivered.isEmpty)
    }

    @Test func payloadSpacingAndCap() {
        var s = AnyAppSession(snapshot: field(), profile: mail)
        #expect(s.payload("Hello") == "Hello", "first delivery: as shaped")
        s.record(utterance: 0, text: "Hello", method: .direct, verified: true)
        #expect(s.payload("world") == " world")
        #expect(s.payload(", and") == ", and", "punctuation attaches")
        #expect(s.payload("\nnext") == "\nnext")
        #expect(!s.exceedsCap(adding: String(repeating: "x", count: AnyAppSession.maxCharsPerGesture - 5)))
        #expect(s.exceedsCap(adding: String(repeating: "x", count: AnyAppSession.maxCharsPerGesture)))
        #expect(s.doneCopy == "✓ 1 line into Mail")
    }

    @Test func rewriteThreadBuilding() {
        var s = AnyAppSession(snapshot: field(), profile: mail, selection: "The quick brown fox")
        #expect(s.entry == .rewrite(chars: 19))
        #expect(s.thread.isEmpty)
        s.refined(instruction: "shorter", result: "Quick fox")
        s.refined(instruction: "more formal", result: "The fox is quick.")
        #expect(s.thread == [["shorter", "Quick fox"], ["more formal", "The fox is quick."]])
    }
}
