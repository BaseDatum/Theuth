import Foundation
import Testing
@testable import CompanionCore

// W6: the key table (item 4), the rail presences (item 3), the live dictation reducer (item 2).

struct KeyTableTests {
    @Test func defaultsAreUniquePerScopeAndRoundTrip() {
        let t = KeyTable()
        #expect(t.isUniquePerScope)
        #expect(t.bindings.count == KeyAction.allCases.count, "every action has exactly one row")
        #expect(Set(t.bindings.map(\.action)).count == KeyAction.allCases.count)
        #expect(KeyTable(json: t.json) == t)
        #expect(t.json == "{}")
        // Same chord across scopes is fine: ⌘O in the panel and the Ask bar.
        #expect(t.chord(for: .openSession) == t.chord(for: .askOpenSession))
    }

    @Test func lookupMatchesCharsForLettersAndCodesForSpecials() {
        let t = KeyTable()
        #expect(t.action(for: Chord(chars: "J", keyCode: 0, modifiers: .shift), in: .panel) == .next, "⇧J is J in the panel")
        #expect(t.action(for: Chord(chars: "o", keyCode: 31, modifiers: .command), in: .panel) == .openSession)
        #expect(t.action(for: Chord(chars: "", keyCode: 125), in: .panel) == .next, "↓ alias")
        #expect(t.action(for: Chord(chars: "\u{3}", keyCode: 76), in: .panel) == .answerRecommended, "keypad ⏎ alias")
        #expect(t.action(for: Chord(chars: "?", keyCode: 44, modifiers: .shift), in: .panel) == .legend, "? alias")
        #expect(t.action(for: Chord(chars: "?", keyCode: 44, modifiers: .shift), in: .askBar) == .askLegend)
        #expect(t.action(for: Chord(chars: "j", keyCode: 38), in: .askBar) == nil, "J means nothing in the Ask bar")
        #expect(t.action(for: Chord(chars: " ", keyCode: 49, modifiers: [.control, .shift]), in: .anywhere) == .openPanel)
        #expect(t.action(for: Chord(chars: "j", keyCode: 38, modifiers: .option), in: .panel) == nil, "⌥J is not J")
    }

    @Test func overrideWinsThenResetRestores() {
        var t = KeyTable()
        t.set(Chord(chars: "n", keyCode: 45), for: .next)
        #expect(t.chord(for: .next)?.symbol == "N")
        #expect(t.action(for: Chord(chars: "n"), in: .panel) == .next)
        #expect(t.action(for: Chord(chars: "j"), in: .panel) == nil, "the old chord is free")
        #expect(KeyGrammar.intent(for: KeyPress(chars: "n"), table: t) == .next)
        #expect(KeyTable(json: t.json) == t, "overrides persist")
        #expect(!t.binding(.next).isDefault)
        t.reset(.next)
        #expect(t == KeyTable())
        t.set(Chord(chars: "x", keyCode: 7), for: .snooze); t.set(nil, for: .clear)
        #expect(t.chord(for: .clear) == nil, "unassigned persists as null")
        #expect(KeyTable(json: t.json).chord(for: .clear) == nil)
        t.resetAll()
        #expect(t == KeyTable())
        t.set(Chord(chars: "J", keyCode: 38, modifiers: .shift), for: .previous)
        #expect(t.overrides[.previous] == .some(Chord(chars: "j", keyCode: 38)), "local chords are stored lower-cased without ⇧")
    }

    @Test func collisionPolicy() {
        var t = KeyTable()
        let j = Chord(chars: "j", keyCode: 38)
        #expect(t.check(j, for: .next) == .unchanged)
        #expect(t.check(j, for: .previous) == .usedBy(.next), "same scope: refused with the owner")
        #expect(t.check(j, for: .askOpenSession) == .ok, "across scopes: allowed")
        t.move(j, to: .previous)
        #expect(t.chord(for: .previous) == j)
        #expect(t.chord(for: .next) == nil, "\"Use here instead\": the other row goes unassigned")
        #expect(t.isUniquePerScope)
        // Global scope: the symbolic hot keys and bare keys are reserved; F-keys alone are fine.
        let spotlight = Chord(chars: " ", keyCode: 49, modifiers: .command)
        #expect(t.check(spotlight, for: .openAskBar, reserved: [spotlight]) == .reservedByMacOS)
        #expect(t.check(Chord(chars: "a", keyCode: 0), for: .openAskBar) == .reservedByMacOS, "no modifier")
        #expect(t.check(Chord(chars: "A", keyCode: 0, modifiers: .shift), for: .openAskBar) == .reservedByMacOS, "⇧ alone does not count")
        #expect(t.check(Chord(chars: "", keyCode: 96), for: .openAskBar) == .ok, "F5")
        #expect(t.check(Chord(chars: " ", keyCode: 49, modifiers: [.control, .shift]), for: .openAskBar) == .usedBy(.openPanel))
        #expect(t.check(Chord(chars: "a", keyCode: 0), for: .snooze) == .ok, "local rows take bare keys")
    }

    @Test func registrationFailureFallsBackToTheDefaultWithANotice() {
        let def = Chord(chars: " ", keyCode: 49, modifiers: [.control, .shift])
        let saved = Chord(chars: "k", keyCode: 40, modifiers: [.control, .option])
        #expect(KeyTable.resolveGlobal(saved: saved, default: def) { _ in true } == .init(chord: saved, notice: nil))
        let r = KeyTable.resolveGlobal(saved: saved, default: def) { $0 != saved }
        #expect(r.chord == def)
        #expect(r.notice == "⌃⌥K is held by another app — using ⌃⇧Space")
        let none = KeyTable.resolveGlobal(saved: saved, default: def) { _ in false }
        #expect(none.chord == nil && none.notice?.contains("no hotkey") == true)
        #expect(KeyTable.resolveGlobal(saved: nil, default: def) { _ in false } == .init(chord: nil, notice: nil), "unassigned registers nothing")
    }

    @Test func chordSymbols() {
        #expect(Chord(chars: " ", keyCode: 49, modifiers: [.control, .shift]).symbol == "⌃⇧Space")
        #expect(Chord(chars: "o", keyCode: 31, modifiers: .command).symbol == "⌘O")
        #expect(Chord(chars: "\r", keyCode: 36).symbol == "⏎")
        #expect(Chord(chars: "\u{1b}", keyCode: 53).symbol == "⎋")
        #expect(Chord(chars: "\u{F704}", keyCode: 122, modifiers: .function).symbol == "fnF1")
        #expect(Chord(chars: "/", keyCode: 44, modifiers: [.command, .option]).symbol == "⌥⌘/")
        #expect(Chord(chars: "", keyCode: 126).symbol == "↑")
    }

    @Test func textFieldsTypeInsteadOfActing() {
        #expect(KeyGrammar.intent(for: KeyPress(chars: "j"), textFieldFocused: true) == .none)
        #expect(KeyGrammar.intent(for: KeyPress(chars: "1"), textFieldFocused: true) == .none)
        #expect(KeyGrammar.intent(for: KeyPress(chars: "\u{1b}", keyCode: 53), textFieldFocused: true) == .escape)
        #expect(KeyGrammar.intent(for: KeyPress(chars: "\r", keyCode: 36), scope: .askBar) == .send)
        #expect(KeyGrammar.intent(for: KeyPress(chars: "/", keyCode: 44, command: true), scope: .askBar) == .legend)
        #expect(KeyGrammar.intent(for: KeyPress(chars: ",", keyCode: 43, command: true)) == .settings)
    }

    // 149 §8: the ★ rows, the six scopes, the per-platform default table.
    @Test func rows149UniqueAcrossSixScopesAndSpaceIsReplyInThePanelOnly() {
        let t = KeyTable()
        #expect(KeyScope.allCases.count == 6)
        #expect(t.isUniquePerScope)
        #expect(t.action(for: .space, in: .panel) == .replyFocus)
        #expect(KeyGrammar.intent(for: KeyPress(chars: " ", keyCode: 49)) == .reply, "space is R's alias (Revision 1 row 18)")
        #expect(KeyGrammar.intent(for: KeyPress(chars: " ", keyCode: 49), textFieldFocused: true) == .none, "typed once the field has focus")
        #expect(t.action(for: .space, in: .askBar) == nil)
        #expect(t.action(for: .space, in: .agents) == nil)
        #expect(t.action(for: Chord(chars: "s", keyCode: 1, modifiers: .control), in: .anywhere) == .voiceKey)
        #expect(t.action(for: Chord(chars: "a", keyCode: 0, modifiers: [.control, .shift]), in: .anywhere) == .openAgents)
        #expect(t.action(for: Chord(chars: "v", keyCode: 9), in: .panel) == .talk)
        #expect(t.action(for: Chord(chars: "v", keyCode: 9), in: .agents) == .agentsTalk, "same chord across scopes")
        #expect(t.action(for: Chord(chars: "M", keyCode: 46, modifiers: [.control, .shift]), in: .island) == .islandStop, "⌃⇧M typed; ⇧ drops in a local scope")
        #expect(t.action(for: .escape, in: .pointAndSay) == .pointEscape)
        #expect(t.binding(.pointEscape).fixed && t.binding(.islandEscape).fixed && t.binding(.agentsEscape).fixed)
        #expect(t.action(for: .down, in: .agents) == .agentsNext, "↓ alias in the agents panel")
        #expect(t.check(Chord(chars: "s", keyCode: 1, modifiers: .control), for: .openAgents) == .usedBy(.voiceKey))
    }

    @Test func perPlatformDefaults() {
        #expect(KeyTable.defaultChord(for: .voiceKey, on: .macOS) == Chord(chars: "s", keyCode: 1, modifiers: .control))
        #expect(KeyTable.defaultChord(for: .voiceKey, on: .windows) == Chord(chars: " ", keyCode: 49, modifiers: [.control, .option]))
        #expect(KeyTable.defaultChord(for: .voiceKey, on: .linux) == KeyTable.defaultChord(for: .voiceKey, on: .windows))
        #expect(KeyTable.defaultChord(for: .voiceKey, on: .macOS) == KeyTable().chord(for: .voiceKey), "macOS is the table")
        #expect(KeyTable.defaultChord(for: .openAgents, on: .windows) == KeyTable().chord(for: .openAgents), "no per-platform row: the table's chord")
        #expect(KeyTable.defaultChord(for: .openAskBar, on: .windows) == nil)
    }

    /// 149g F1: ⎋ while the voice key is held arrives with the chord's modifiers; the table derives that ⎋ from the chord.
    @Test func escapeDuringHoldCarriesTheVoiceKeysModifiers() {
        func esc(_ m: Chord.Modifiers) -> Chord { Chord(chars: "\u{1b}", keyCode: 53, modifiers: m) }
        let held = KeyTable.escapeDuringHold(of: KeyTable().chord(for: .voiceKey)!)
        #expect(held == esc(.control) && held?.symbol == "⌃⎋", "the default ⌃S → ⌃⎋")
        #expect(KeyTable.escapeDuringHold(of: Chord(chars: " ", keyCode: 49, modifiers: [.control, .option])) == esc([.control, .option]), "a rebound ⌃⌥Space → ⌃⌥⎋")
        #expect(KeyTable.escapeDuringHold(of: Chord(chars: "s", keyCode: 1, modifiers: [.command, .shift])) == esc([.command, .shift]), "⌘ only because the chord has it")
        #expect(KeyTable.escapeDuringHold(of: Chord(chars: "\u{F704}", keyCode: 122)) == nil, "a bare F1: plain ⎋ already matches")
        #expect(KeyTable.escapeDuringHold(of: Chord(chars: "s", keyCode: 1, modifiers: [.command, .option])) == nil, "⌘⌥⎋ is Force Quit")
        #expect(KeyTable.escapeDuringHold(of: Chord(chars: "s", keyCode: 1, modifiers: [.command, .option, .shift])) == nil, "⌘⌥⇧⎋ force-quits the front app")
        let cmdCtl = Chord(chars: "s", keyCode: 1, modifiers: [.control, .command])
        #expect(KeyTable.escapeDuringHold(of: cmdCtl, reserved: [esc([.control, .command])]) == nil, "an enabled symbolic hot key is the system's")
        #expect(KeyTable.escapeDuringHold(of: cmdCtl, reserved: [esc(.command), Chord(chars: " ", keyCode: 49, modifiers: [.control, .command])]) == esc([.control, .command]),
                "reserved matches the whole chord, not its modifiers or key alone")
    }
}

struct RailPresenceTests {
    @Test func widths() {
        #expect(RailCollapse.width(collapsed: true) == 4)
        #expect(RailCollapse.width(collapsed: false) == 26)
    }

    @Test func sliverWhenQuietIsW1() {
        #expect(RailCollapse.collapseDelay(.sliverWhenQuiet, quiet: true, hovering: false, afterHover: false) == 8)
        #expect(RailCollapse.collapseDelay(.sliverWhenQuiet, quiet: false, hovering: false, afterHover: false) == nil)
        #expect(RailCollapse.collapseDelay(.sliverWhenQuiet, quiet: true, hovering: true, afterHover: false) == nil)
        #expect(RailCollapse.expandsOnActivity(.sliverWhenQuiet))
    }

    @Test func expandedNeverCollapses() {
        #expect(RailCollapse.collapseDelay(.expanded, quiet: true, hovering: false, afterHover: true) == nil)
        #expect(!RailCollapse.expandsOnActivity(.expanded))
    }

    @Test func alwaysSliverIgnoresNeedsYou() {
        #expect(RailCollapse.collapseDelay(.alwaysSliver, quiet: false, hovering: false, afterHover: false) == 0, "needs-you or working: still a sliver")
        #expect(RailCollapse.collapseDelay(.alwaysSliver, quiet: true, hovering: false, afterHover: false) == 0)
        #expect(RailCollapse.collapseDelay(.alwaysSliver, quiet: false, hovering: true, afterHover: false) == nil, "hover expands it")
        #expect(RailCollapse.collapseDelay(.alwaysSliver, quiet: false, hovering: false, afterHover: true) == 1, "pointer left: 1 s linger")
        #expect(!RailCollapse.expandsOnActivity(.alwaysSliver), "a new needs-you item does not expand it")
    }
}

struct AskDictationTests {
    @Test func startLiveStopIdle() {
        var d = AskDictation()
        #expect(!d.isActive)
        let t1 = d.handle(.start); #expect(t1)
        #expect(d.phase == .starting)
        let t2 = d.handle(.text(id: "x", utterance: 0, text: "early", final: false)); #expect(!t2, "not ours yet")
        _ = d.handle(.started(id: "x"))
        #expect(d.phase == .live(id: "x"))
        let t3 = d.handle(.text(id: "x", utterance: 0, text: "ship", final: false)); #expect(t3)
        #expect(d.liveText == "ship" && d.partialText == "ship" && d.finalText == "")
        _ = d.handle(.text(id: "x", utterance: 0, text: "ship it", final: true))
        _ = d.handle(.text(id: "x", utterance: 1, text: "to", final: false))
        #expect(d.finalText == "ship it" && d.partialText == "to" && d.liveText == "ship it to")
        _ = d.handle(.text(id: "x", utterance: 1, text: "", final: true))
        #expect(d.liveText == "ship it", "an empty closed utterance is dropped")
        let t4 = d.handle(.text(id: "other", utterance: 0, text: "no", final: true)); #expect(!t4, "another frontend's dictation")
        let t5 = d.handle(.level(id: "x", levels: [0.1, 0.5])); #expect(!t5)
        #expect(d.levels == [0.1, 0.5])
        _ = d.handle(.stop)
        #expect(d.phase == .stopping(id: "x"))
        let t6 = d.handle(.finished(id: "x", error: nil)); #expect(t6)
        #expect(d.phase == .idle && d.error == nil && d.levels.isEmpty)
    }

    @Test func typedEditsWinOverLatePartials() {
        var d = AskDictation()
        _ = d.handle(.start); _ = d.handle(.started(id: "x"))
        _ = d.handle(.text(id: "x", utterance: 0, text: "ship", final: false))
        _ = d.handle(.stop)
        _ = d.handle(.typed)
        let t7 = d.handle(.text(id: "x", utterance: 0, text: "ship it", final: true)); #expect(!t7, "the field keeps the user's text")
        let t8 = d.handle(.finished(id: "x", error: nil)); #expect(!t8)
        #expect(d.liveText == "ship it", "the reducer still tracks it")
        let t9 = d.handle(.start); #expect(t9, "a new hold resets")
        #expect(d.utterances.isEmpty)
    }

    @Test func errorsSurfaceInTheHintRow() {
        var d = AskDictation()
        _ = d.handle(.start)
        _ = d.handle(.startFailed("already dictating"))
        #expect(d.error == "dictating elsewhere" && d.phase == .idle)
        _ = d.handle(.start)
        _ = d.handle(.startFailed("voice is disabled"))
        #expect(d.error == "voice is disabled")
        _ = d.handle(.start); _ = d.handle(.started(id: "y"))
        #expect(d.error == nil, "a new start clears it")
        _ = d.handle(.finished(id: "y", error: "model failed"))
        #expect(d.error == "model failed" && !d.isActive)
        let t10 = d.handle(.started(id: "z")); #expect(!t10, "a stray start event while idle is ignored")
    }
}
