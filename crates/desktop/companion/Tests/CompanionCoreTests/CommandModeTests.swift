import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 153 §16 rows 17–18: the command card reducer, the spoken approval grammar, the one answer card's state rules.

struct SpokenApprovalTests {
    @Test func yesAndGoAllow() {
        #expect(SpokenApproval.decide("yes") == .allow)
        #expect(SpokenApproval.decide("Yes.") == .allow)
        #expect(SpokenApproval.decide("go") == .allow)
        #expect(SpokenApproval.decide("Go ahead!") == .allow)
        #expect(SpokenApproval.decide("do it") == .allow)
    }

    @Test func noDenies() {
        #expect(SpokenApproval.decide("no") == .deny)
        #expect(SpokenApproval.decide("Nope") == .deny)
        #expect(SpokenApproval.decide("cancel") == .deny)
    }

    /// §7.1 tolerance `len / 5`, phrases ≥ 5 chars only: "approved" is one edit from "approve"; "yet" is not "yes".
    @Test func nearMissOnlyForLongPhrases() {
        #expect(SpokenApproval.decide("approved") == .allow)
        #expect(SpokenApproval.decide("go a head") == .allow)
        #expect(SpokenApproval.decide("cancer") == .deny)
        #expect(SpokenApproval.decide("yet") == nil)
        #expect(SpokenApproval.decide("nah") == nil)
    }

    @Test func unknownIsIgnored() {
        #expect(SpokenApproval.decide("make it blue") == nil)
        #expect(SpokenApproval.decide("") == nil)
        #expect(SpokenApproval.decide("yes but only the first one") == nil)
    }

    @Test func constants() {
        #expect(SpokenApproval.listenSecs == 8)
        #expect(SpokenApproval.levenshtein("kitten", "sitting") == 3)
    }
}

struct CommandCardTests {
    typealias E = CommandCard.Event

    @Test func rowsWithAndWithoutPurpose() {
        var c = CommandCard(request: "move the screenshots")
        c.observe(.toolCallStarted(id: "1", tool: "shell", purpose: "checking: ls ~/Desktop/*.png"))
        c.observe(.toolCallStarted(id: "2", tool: "applescript", purpose: nil))
        c.observe(.toolCallStarted(id: "3", tool: "shell", purpose: "Executing: mkdir -p ~/Desktop/Shots"))
        #expect(c.rows.map(\.kind) == [.checking, .result, .executing])
        #expect(c.rows.map(\.text) == ["ls ~/Desktop/*.png", "applescript", "mkdir -p ~/Desktop/Shots"])
        #expect(c.tools == ["shell", "applescript", "shell"])
        #expect(c.rows.map { $0.label(queued: false) } == ["checking", "running", "executing"])
        c.observe(.toolCallFinished(id: "1", ok: true))
        c.observe(.toolCallFinished(id: "2", ok: false))
        #expect(c.rows.map(\.ok) == [true, false, nil])
        #expect(c.rows[0].label(queued: false) == "checked" && c.rows[1].label(queued: false) == "ran")
        #expect(c.rows[2].label(queued: true) == "next")
        // A purpose over 60 chars is clipped; an unknown prefix is the text itself.
        let long = "verifying: " + String(repeating: "x", count: 80)
        let (kind, text) = CommandCard.parsePurpose(long, tool: "shell")
        #expect(kind == .verifying && text.count == CommandCard.textCap && text.hasSuffix("…"))
        #expect(CommandCard.parsePurpose("looking around", tool: "shell") == (.result, "looking around"))
        #expect(CommandCard.parsePurpose("checking:", tool: "shell") == (.checking, "shell"))
    }

    @Test func capAtTwelveFreezes() {
        var c = CommandCard()
        var hit: [Int] = []
        for i in 1...14 where c.observe(.toolCallStarted(id: "\(i)", tool: "shell", purpose: nil)) { hit.append(i) }
        #expect(hit == [CommandCard.maxSteps])
        #expect(c.rows.count == CommandCard.maxSteps && c.steps == CommandCard.maxSteps)
        #expect(c.frozen == CommandCard.stoppedCopy && c.finished)
        // The cancel's `TurnFinished { cancelled }` and late text do not overwrite the stopped copy.
        c.observe(.textDelta("✗ cancelled"))
        c.observe(.turnFinished(stopReason: "cancelled"))
        #expect(c.frozen == CommandCard.stoppedCopy && c.verdict == nil && c.text.isEmpty)
        // Late finishes still colour their rows.
        c.observe(.toolCallFinished(id: "12", ok: true))
        #expect(c.rows.last?.ok == true)
    }

    @Test func verdictParse() {
        #expect(CommandCard.parseVerdict("✓ 14 screenshots moved") == .ok("14 screenshots moved"))
        #expect(CommandCard.parseVerdict("\n✗ couldn't find Desktop/Shots\n") == .failed("couldn't find Desktop/Shots"))
        #expect(CommandCard.parseVerdict("Sure, I moved them.") == .replied("Sure, I moved them."))
        var c = CommandCard()
        c.observe(.textDelta("✓ done"))
        c.observe(.textDelta(", 3 files"))
        c.observe(.turnFinished(stopReason: "end_turn"))
        #expect(c.verdict == .ok("done, 3 files") && c.finished)
        var e = CommandCard()
        e.observe(.turnFinished(stopReason: "error"))
        #expect(e.frozen == "✗ couldn't finish — the turn failed" && e.verdict == nil)
    }
}

struct AnswerCardStateTests {
    @Test func staleHideIgnored() {
        var s = AnswerCardState()
        s.begin(question: "one")
        let g1 = s.generation
        s.begin(question: "two")
        #expect(s.hide(generation: g1) == false && s.visible)
        #expect(s.hide(generation: s.generation) && !s.visible)
        s.followUp(question: "three")
        #expect(s.visible && s.turns.count == 2 && s.current == 1 && s.generation == g1 + 2)
        #expect(s.hide(generation: g1 + 1) == false && s.visible)
    }

    @Test func doneArmsOnlyCollapsedNotSpeakingFinished() {
        var s = AnswerCardState()
        s.begin(question: "q")
        #expect(!s.armsDone, "thinking")
        s.phase = .answer
        #expect(!s.armsDone, "turn not finished")
        s.turnFinished = true
        #expect(s.armsDone)
        s.speaking = true
        #expect(!s.armsDone)
        s.speaking = false; s.expanded = true
        #expect(!s.armsDone)
        s.expanded = false; s.phase = .failed
        #expect(!s.armsDone, "failed never auto-hides")
        s.phase = .answer
        s.approval = .init(id: "a", tool: "shell", target: "rm -rf build", risk: "destructive")
        #expect(!s.armsDone, "an approval box holds the card")
        s.approval?.outcome = .denied
        #expect(s.armsDone)
        #expect(AnswerCardState.doneMs == 6_000)
    }

    @Test func commandTurnsCarryTheReducerAndMarkers() {
        var s = AnswerCardState()
        s.begin(question: "move them", kind: .command)
        #expect(s.command?.request == "move them")
        s.command?.observe(.toolCallStarted(id: "1", tool: "shell", purpose: "checking: ls"))
        #expect(s.turns[0].command?.rows.count == 1)
        s.followUp(question: "and the rest")
        #expect(s.command?.rows.isEmpty == true && s.turns[0].command?.rows.count == 1)
        s.move(-1); #expect(s.current == 0)
        s.move(-1); #expect(s.current == 0)
        s.move(5); #expect(s.current == 1)
        #expect(AnswerCardState.marker(.command, durationMs: 3_200) == "[command, 4 s] ")
        #expect(AnswerCardState.marker(.voiceNote, durationMs: 0) == "[voice note, 1 s] ")
        #expect(AnswerCardState.marker(.ask, durationMs: 0) == "")
    }
}
