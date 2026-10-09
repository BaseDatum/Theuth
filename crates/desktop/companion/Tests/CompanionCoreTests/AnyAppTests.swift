import Testing
import Foundation
import TheuthKit
@testable import CompanionCore

/// 153-D1 §13.1 (§16 row 23): every role of the table, the secure rule, the edge cases.
struct DeliveryAssessmentTests {
    typealias V = DeliveryAssessment.Verdict
    func v(_ role: String?, subrole: String? = nil, editable: Bool? = nil, secure: Bool = false,
           payload: DeliveryAssessment.Payload = .text, terminal: Bool = false) -> V {
        DeliveryAssessment.assess(AXSnapshot(role: role, subrole: subrole, isEditable: editable, isSecure: secure),
                                  payload: payload, terminal: terminal)
    }

    @Test func everyRoleOfTheTable() {
        for r in ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"] { #expect(v(r) == .editable, "\(r)") }
        for r in ["AXGroup", "AXWebArea", "AXWindow", "AXScrollArea", "AXUnknown"] { #expect(v(r) == .unknown, "\(r)") }
        #expect(v(nil) == .unknown, "no focused element readable")
        for r in ["AXButton", "AXMenu", "AXMenuBar", "AXMenuItem", "AXMenuButton", "AXStaticText", "AXSlider", "AXCheckBox",
                  "AXRadioButton", "AXImage", "AXToolbar", "AXRow"] {
            #expect(v(r) == .certainlyNot(.role(r)), "\(r)")
        }
    }

    @Test func secureFieldTakesTextNeverReturn() {
        #expect(v("AXSecureTextField") == .editable)
        #expect(v("AXTextField", secure: true) == .editable)
        #expect(v("AXSecureTextField", payload: .returnKey) == .certainlyNot(.secure))
        #expect(v("AXTextField", subrole: "AXSecureTextField", payload: .returnKey) == .certainlyNot(.secure))
        #expect(v("AXTextField", payload: .returnKey) == .editable, "Return into a plain field is fine")
        #expect(AXSnapshot(role: "AXTextField", subrole: "AXSecureTextField").secure)
    }

    @Test func edgeCases() {
        #expect(v("AXTextField", subrole: "AXSearchField") == .editable, "search field as a subrole")
        #expect(v("AXGroup", editable: true) == .editable, "kAXValue settable on an unknown role")
        #expect(v("AXSlider", editable: true) == .certainlyNot(.role("AXSlider")), "a settable value does not make a slider a text target")
        #expect(v("AXTextField", editable: false) == .unknown, "read-only field: not certain enough to refuse")
        #expect(v("AXTextArea", terminal: true) == .unknown, "terminal profile: lease, never direct insertion")
        #expect(v("AXGroup", editable: true, terminal: true) == .unknown)
        #expect(v("AXButton", terminal: true) == .certainlyNot(.role("AXButton")), "the profile never un-refuses")
    }

    @Test func hintLabels() {
        #expect(DeliveryAssessment.label(.role("AXButton")) == "button")
        #expect(DeliveryAssessment.label(.role("AXStaticText")) == "static text")
        #expect(DeliveryAssessment.label(.role("AXMenuItem")) == "menu item")
        #expect(DeliveryAssessment.label(.secure) == "secure field")
    }
}

/// An in-memory `NSPasteboard`: items + changeCount; the user's copy is `userCopies`.
final class FakePasteboard: Pasteboard {
    var items: [PasteboardItem]
    var changeCount = 7
    var failSnapshot = false, failWrite = false, failRestore = false
    var restored: [[PasteboardItem]] = []
    struct Boom: Error {}
    init(_ items: [PasteboardItem]) { self.items = items }

    func snapshot() throws -> [PasteboardItem] { if failSnapshot { throw Boom() }; return items }
    func write(_ new: [PasteboardItem]) throws -> Int {
        if failWrite { throw Boom() }
        items = new; changeCount += 1; return changeCount
    }
    func restore(_ old: [PasteboardItem]) throws {
        if failRestore { throw Boom() }
        items = old; changeCount += 1; restored.append(old)
    }
    func userCopies(_ s: String) { items = [PasteboardItem([ClipboardLease.textType: Data(s.utf8)])]; changeCount += 1 }
}

/// 153-D1 §13.3 (§16 row 23): settle only if still ours; stale generation no-op; the setting skips the restore.
struct ClipboardLeaseTests {
    let users = [PasteboardItem([ClipboardLease.textType: Data("mine".utf8), "public.rtf": Data([1, 2, 3])]),
                 PasteboardItem(["public.file-url": Data("file:///x".utf8)])]

    @Test func restoresIfStillOurs() throws {
        let pb = FakePasteboard(users)
        var lease = ClipboardLease(pasteboard: pb)
        let g = try lease.begin()
        #expect(lease.snapshot == users && lease.phase == .held)
        try lease.write("hello there", session: "g1")
        let ours = pb.items[0]
        #expect(String(decoding: ours[ClipboardLease.textType]!, as: UTF8.self) == "hello there")
        #expect(String(decoding: ours[ClipboardLease.sessionType]!, as: UTF8.self) == "g1")
        for m in ClipboardLease.markerTypes { #expect(ours[m] != nil, "\(m)") }
        lease.pasteIssued()
        #expect(lease.phase == .pasted)
        #expect(try lease.settle(generation: g) == .restored)
        #expect(pb.items == users && pb.restored.count == 1 && lease.phase == .settled)
        #expect(try lease.settle(generation: g) == .nothing, "a second settle has nothing to do")
    }

    @Test func notIfTheUserCopied() throws {
        let pb = FakePasteboard(users)
        var lease = ClipboardLease(pasteboard: pb)
        let g = try lease.begin()
        try lease.write("hello", session: "g1"); lease.pasteIssued()
        pb.userCopies("theirs")
        #expect(try lease.settle(generation: g) == .keptUsers)
        #expect(String(decoding: pb.items[0][ClipboardLease.textType]!, as: UTF8.self) == "theirs" && pb.restored.isEmpty)

        // Same changeCount but the item is not ours any more (an app rewrote in place): also left alone.
        let pb2 = FakePasteboard(users)
        var lease2 = ClipboardLease(pasteboard: pb2)
        let g2 = try lease2.begin()
        try lease2.write("hello", session: "g2"); lease2.pasteIssued()
        pb2.items = [PasteboardItem([ClipboardLease.textType: Data("swapped".utf8)])]
        #expect(try lease2.settle(generation: g2) == .keptUsers && pb2.restored.isEmpty)
    }

    @Test func staleGenerationIsANoOp() throws {
        let pb = FakePasteboard(users)
        var lease = ClipboardLease(pasteboard: pb)
        let g1 = try lease.begin()
        try lease.write("one", session: "g1"); lease.pasteIssued()
        let g2 = try lease.begin()
        #expect(g2 == g1 + 1 && lease.snapshot == users, "a begin over our own unsettled item keeps the user's snapshot")
        try lease.write("two", session: "g2"); lease.pasteIssued()
        #expect(try lease.settle(generation: g1) == .stale)
        #expect(String(decoding: pb.items[0][ClipboardLease.textType]!, as: UTF8.self) == "two", "the live lease's item is untouched")
        #expect(try lease.settle(generation: g2) == .restored)
        #expect(pb.items == users, "the user's clipboard, not gesture one's text, comes back")
    }

    @Test func leaveTextSkipsTheRestore() throws {
        let pb = FakePasteboard(users)
        var lease = ClipboardLease(pasteboard: pb, leaveTextOnClipboard: true)
        let g = try lease.begin()
        try lease.write("keep me", session: "g1"); lease.pasteIssued()
        #expect(try lease.settle(generation: g) == .leftText)
        #expect(String(decoding: pb.items[0][ClipboardLease.textType]!, as: UTF8.self) == "keep me" && pb.restored.isEmpty)
    }

    @Test func failuresAreTyped() throws {
        let pb = FakePasteboard(users)
        var lease = ClipboardLease(pasteboard: pb)
        pb.failSnapshot = true
        #expect(throws: ClipboardLease.Failure.self) { try lease.begin() }
        pb.failSnapshot = false; pb.failWrite = true
        let g = try lease.begin()
        #expect(throws: ClipboardLease.Failure.self) { try lease.write("x", session: "g") }
        #expect(try lease.settle(generation: g) == .nothing, "nothing written, nothing to restore")
        pb.failWrite = false; pb.failRestore = true
        let g2 = try lease.begin()
        try lease.write("x", session: "g2"); lease.pasteIssued()
        #expect(throws: ClipboardLease.Failure.self) { try lease.settle(generation: g2) }
    }

    @Test func oversizedSnapshotKeepsTextTypesOnly() {
        let big = Data(count: ClipboardLease.maxRepresentationBytes + 1)
        let items = [PasteboardItem([ClipboardLease.textType: Data("t".utf8), "public.tiff": big]),
                     PasteboardItem(["public.png": Data([1])])]
        #expect(ClipboardLease.capped(items) == [PasteboardItem([ClipboardLease.textType: Data("t".utf8)])])
        #expect(ClipboardLease.capped(users) == users, "under the cap: everything")
    }
}

/// 153-D1 §13.4 (§16 row 23): focus moved → unknown; the user acted → unknown; all unchanged → notLanded; our text → landed.
struct PasteVerifierTests {
    let before = FieldRead(value: "ls -la", count: 6, caret: 6)

    @Test func focusMovedIsUnknown() {
        var v = PasteVerifier(text: "git status", before: before)
        #expect(v.observe(before) == nil, "150 ms: nothing yet")
        #expect(v.observe(nil) == .unknown(.focusMoved))
        #expect(v.observe(FieldRead(value: "ls -lagit status", count: 16, caret: 16)) == .unknown(.focusMoved), "decided; later reads ignored")
    }

    @Test func userActedIsUnknown() {
        #expect(PasteVerifier.verify(text: "git status", before: before,
                                     reads: [before, FieldRead(value: "ls -lax", count: 7, caret: 7)]) == .unknown(.userActed))
        #expect(PasteVerifier.verify(text: "git status", before: before,
                                     reads: [FieldRead(value: "ls -la", count: 6, caret: 2)]) == .unknown(.userActed), "caret moved only")
    }

    @Test func allUnchangedIsNotLanded() {
        var v = PasteVerifier(text: "git status", before: before)
        #expect(v.observe(before) == nil && v.observe(before) == nil)
        #expect(v.observe(before) == .notLanded)
        #expect(PasteVerifier.verify(text: "x", before: before, reads: [before]) == .notLanded, "direct insertion: one read-back")
        #expect(PasteVerifier(text: "x", before: before, reads: 3).reads == 3)
    }

    @Test func ourTextLands() {
        #expect(PasteVerifier.verify(text: "git status", before: before,
                                     reads: [FieldRead(value: "ls -lagit status", count: 16, caret: 16)]) == .landed)
        // The field already held the words once: one more occurrence is the paste, the same count is not.
        let twice = FieldRead(value: "the the", count: 7, caret: 7)
        #expect(PasteVerifier.verify(text: "the", before: FieldRead(value: "the", count: 3, caret: 3), reads: [twice]) == .landed)
        #expect(PasteVerifier.verify(text: "the", before: twice, reads: [FieldRead(value: "the them", count: 8, caret: 8)]) == .unknown(.userActed))
        // No readable value (count-only element): a longer field is a landing.
        #expect(PasteVerifier.verify(text: "abc", before: FieldRead(count: 10), reads: [FieldRead(count: 13)]) == .landed)
        #expect(PasteVerifier.verify(text: "abc", before: FieldRead(count: 10), reads: [FieldRead(count: 10), FieldRead(count: 10), FieldRead(count: 10)]) == .notLanded)
    }

    @Test func runUsesTheInjectedClock() async {
        var slept: [Int] = [], n = 0
        let after = FieldRead(value: "ls -lagit status", count: 16, caret: 16)
        let before = before
        let o = await PasteVerifier.run(text: "git status", before: before, sleep: { slept.append($0) },
                                        read: { n += 1; return n == 3 ? after : before })
        #expect(o == .landed && slept == [150, 200, 650] && n == 3)
        slept = []
        let o2 = await PasteVerifier.run(text: "git status", before: before, sleep: { slept.append($0) }, read: { nil })
        #expect(o2 == .unknown(.focusMoved) && slept == [150], "stops at the first decision")
    }
}

/// 153-D1 §13 / §9.1: profile resolution — by bundle id, then `any_app`, then the terminal heuristics.
struct AppProfilesTests {
    let rows = [AppProfile(id: "com.apple.mail", name: "Mail", polish: true, learn: true, commands: ["send"]),
                AppProfile(id: AppProfiles.anyAppId, name: "Other apps", polish: false, learn: true)]

    @Test func resolution() {
        #expect(AppProfiles.resolve(bundleId: "com.apple.mail", in: rows) == rows[0])
        let slack = AppProfiles.resolve(bundleId: "com.tinyspeck.slackmacgap", name: "Slack", in: rows)
        #expect(slack.id == "com.tinyspeck.slackmacgap" && slack.name == "Slack" && slack.learn && !slack.polish && !slack.terminal,
                "the any_app row, re-labelled for the app")
        let term = AppProfiles.resolve(bundleId: "com.apple.Terminal", name: "Terminal", in: rows)
        #expect(term.terminal && term.learn, "any_app row + terminal heuristics")
        let bare = AppProfiles.resolve(bundleId: "com.mitchellh.ghostty", name: "Ghostty", in: [])
        #expect(bare == AppProfile(id: "com.mitchellh.ghostty", name: "Ghostty", terminal: true), "no rows: synthesized")
        #expect(AppProfiles.resolve(bundleId: "com.apple.Notes", in: []) == AppProfile(id: "com.apple.Notes", name: "com.apple.Notes"))
        #expect(AppProfiles.resolve(bundleId: nil, in: rows).id == AppProfiles.anyAppId)
        let pinned = AppProfiles.resolve(bundleId: "com.apple.Terminal", in: [AppProfile(id: "com.apple.Terminal", name: "T", terminal: false)])
        #expect(!pinned.terminal, "an explicit row wins over the heuristics")
    }

    @Test func terminalHeuristics() {
        for id in ["com.apple.Terminal", "com.googlecode.iterm2", "com.mitchellh.ghostty", "dev.warp.Warp-Stable", "net.kovidgoyal.kitty",
                   "org.alacritty", "com.github.wez.wezterm", "com.microsoft.VSCode", "com.todesktop.230313mzl4w4u92", "dev.zed.Zed"] {
            #expect(AppProfiles.isTerminal(bundleId: id), "\(id)")
        }
        #expect(!AppProfiles.isTerminal(bundleId: "com.apple.mail") && !AppProfiles.isTerminal(bundleId: nil))
    }

    @Test func roundTripsThroughModeKeySettings() {
        let d = UserDefaults(suiteName: "153-D1-\(UUID().uuidString)")!
        ModeKeySettings.setAppProfiles(rows, d)
        #expect(ModeKeySettings.appProfiles(d) == rows)
        #expect(AppProfiles.resolve(bundleId: "com.apple.mail", in: ModeKeySettings.appProfiles(d)).commands == ["send"])
    }
}
