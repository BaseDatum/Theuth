import Testing
import Foundation
import TheuthKit
@testable import CompanionCore

private let t0 = Date(timeIntervalSince1970: 1_791_448_800)

/// 153-B3: "send it" (§7.4), the mic at key-down (§8.5), the correction id (§6.1) on the pure machines.
struct AskVoiceMachineTests {
    /// §7.4: a finish carrying "send it" lands as `.sendNow` — the controller commits with no 3 s hold. Both ways the
    /// dictation ends: the release (`.stopping`) and the engine's own settle while the key is still held (`.recording`).
    @Test func sendItSkipsTheHold() {
        var m = AskVoiceMachine()
        _ = m.handle(.keyDown(t0)); _ = m.handle(.tick(t0.addingTimeInterval(0.3)))
        #expect(m.handle(.keyUp(t0.addingTimeInterval(2))) == [.stopDictation])
        #expect(m.handle(.dictated(" ship it ", send: true)) == [.sendNow("ship it")])
        #expect(m.state == .idle)

        _ = m.handle(.keyDown(t0)); _ = m.handle(.tick(t0.addingTimeInterval(0.3)))
        #expect(m.isRecording)
        #expect(m.handle(.dictated("ship it", send: true)) == [.sendNow("ship it")], "the engine settled it while the key is held")
        #expect(m.handle(.keyUp(t0.addingTimeInterval(5))) == [], "the release after the engine's finish is nothing")
        #expect(m.handle(.dictated("", send: true)) == [], "nothing pending")

        _ = m.handle(.keyDown(t0)); _ = m.handle(.tick(t0.addingTimeInterval(0.3))); _ = m.handle(.keyUp(t0.addingTimeInterval(1)))
        #expect(m.handle(.dictated("plain", send: false)) == [.setText("plain")], "without the phrase: the hold as before")
    }

    /// §8.5: with the setting on, key-down opens the mic; a tap discards it and never starts a dictation; a hold starts the
    /// dictation without opening the mic twice. Off (the default of the pure machine) nothing changes.
    @Test func openMicDiscardsOnTap() {
        var m = AskVoiceMachine()
        m.micOnKeyDown = true
        #expect(m.handle(.keyDown(t0)) == [.openMic])
        #expect(m.pressedAt == t0 && m.isPressed)
        #expect(m.handle(.tick(t0.addingTimeInterval(0.1))) == [])
        #expect(m.handle(.keyUp(t0.addingTimeInterval(0.2))) == [.discardMic, .tap])
        #expect(m.state == .idle)

        #expect(m.handle(.keyDown(t0)) == [.openMic])
        #expect(m.handle(.tick(t0.addingTimeInterval(0.25))) == [.startRecording], "the mic is already open")
        #expect(m.handle(.keyUp(t0.addingTimeInterval(1))) == [.stopDictation])
        #expect(m.handle(.dictated("hi")) == [.setText("hi")])

        _ = m.handle(.keyDown(t0))
        #expect(m.handle(.failed("microphone access denied")) == [.fail("microphone access denied")], "a refusal while pressed ends the gesture")

        m.micOnKeyDown = false
        #expect(m.handle(.keyDown(t0)) == [])
        #expect(m.handle(.keyUp(t0.addingTimeInterval(0.1))) == [.tap], "no mic was opened, nothing to discard")
    }

    /// §6.1: the finished dictation's id waits as `correctionId` until the text goes out (`.settled`), the field is cleared, or
    /// the next dictation starts; an error finish leaves nothing to correct.
    @Test func correctionSentOnCommit() {
        var d = AskDictation()
        _ = d.handle(.start); _ = d.handle(.started(id: "a"))
        _ = d.handle(.text(id: "a", utterance: 0, text: "parakeet tee dee tee", final: true))
        #expect(d.correctionId == nil, "nothing to correct while live")
        _ = d.handle(.stop); _ = d.handle(.finished(id: "a", error: nil))
        #expect(d.correctionId == "a" && !d.isActive)
        _ = d.handle(.typed)
        #expect(d.correctionId == "a", "an edit is exactly what the correction reports")
        _ = d.handle(.settled)
        #expect(d.correctionId == nil)

        _ = d.handle(.start); _ = d.handle(.started(id: "b")); _ = d.handle(.finished(id: "b", error: "model failed"))
        #expect(d.correctionId == nil && d.error == "model failed")

        _ = d.handle(.start); _ = d.handle(.started(id: "c")); _ = d.handle(.finished(id: "c", error: nil))
        #expect(d.correctionId == "c")
        _ = d.handle(.start)
        #expect(d.correctionId == nil, "a new dictation supersedes the old id")
    }

    /// §7: "send it" on any piece arms `send` until the finish; `DictationDevice` is a hint while live and clears at the finish.
    @Test func sendArmsAndDeviceHints() {
        var d = AskDictation()
        _ = d.handle(.start); _ = d.handle(.started(id: "a"))
        _ = d.handle(.text(id: "a", utterance: 0, text: "ship it", final: false, command: .send(nearMiss: false)))
        #expect(d.send)
        _ = d.handle(.text(id: "a", utterance: 0, text: "ship it", final: true, command: nil))
        #expect(d.send, "sticky until the finish")
        _ = d.handle(.device(id: "a", device: nil, state: .lost))
        #expect(d.deviceHint == "microphone lost — retrying…")
        _ = d.handle(.device(id: "a", device: "MacBook Pro Microphone", state: .changed))
        #expect(d.deviceHint == "mic: MacBook Pro Microphone")
        _ = d.handle(.device(id: "zzz", device: nil, state: .lost))
        #expect(d.deviceHint == "mic: MacBook Pro Microphone", "another dictation's device is not ours")
        _ = d.handle(.finished(id: "a", error: nil))
        #expect(d.send && d.deviceHint == nil && d.liveText == "ship it")
        _ = d.handle(.start)
        #expect(!d.send)
    }
}

/// 153 §14.4: `TheuthKit.DictationWire` → `AskDictation.Event` — `shaped ?? text`, `command`, `DictationDevice`, the scratch.
struct DictationWireTests {
    private func event(_ json: String) -> AskDictation.Event? {
        AskDictation.event(try! TheuthJSON.decoder.decode(DictationWire.self, from: Data(json.utf8)))
    }

    @Test func shapedWinsOverRawAndCommandRides() {
        #expect(event(#"{"type":"DictationText","id":"d1","utterance":0,"text":"new line ship it","final":false,"shaped":"\n","command":{"command":"send","near_miss":false},"actions":["new_line"]}"#)
            == .text(id: "d1", utterance: 0, text: "\n", final: false, command: .send(nearMiss: false)))
        #expect(event(#"{"type":"DictationText","id":"d1","utterance":1,"text":"scratch that","final":false,"shaped":"","command":{"command":"scratch_that"}}"#)
            == .text(id: "d1", utterance: 1, text: "", final: false, command: .scratchThat))
        #expect(event(#"{"type":"DictationText","id":"d1","utterance":0,"text":"pre 153 engine","final":true}"#)
            == .text(id: "d1", utterance: 0, text: "pre 153 engine", final: true, command: nil), "no `shaped`: the raw text")
        #expect(event(#"{"type":"DictationDevice","id":"d1","device":null,"state":"lost"}"#) == .device(id: "d1", device: nil, state: .lost))
        #expect(event(#"{"type":"DictationStarted","id":"d1"}"#) == nil, "149g: ours is the dictation_start reply")
        #expect(event(#"{"type":"DictationFinished","id":"d1","reason":{"reason":"error","message":"model missing"}}"#) == .finished(id: "d1", error: "model missing"))
        #expect(event(#"{"type":"DictationFinished","id":"d1","reason":{"reason":"done"}}"#) == .finished(id: "d1", error: nil))
        #expect(event(#"{"type":"DictationLevel","id":"d1","levels":[0.5]}"#) == .level(id: "d1", levels: [0.5]))
    }

    /// §4.6: "scratch that" is the previous closed utterance re-sent empty — the field's splice empties, nothing else moves.
    @Test func scratchEmptiesThePreviousUtterance() {
        var d = AskDictation()
        _ = d.handle(.start); _ = d.handle(.started(id: "d1"))
        _ = d.handle(event(#"{"type":"DictationText","id":"d1","utterance":0,"text":"ship the fix","final":true,"shaped":"Ship the fix."}"#)!)
        _ = d.handle(event(#"{"type":"DictationText","id":"d1","utterance":1,"text":"wait no","final":true,"shaped":"Wait, no."}"#)!)
        #expect(d.finalText == "Ship the fix. Wait, no.")
        _ = d.handle(event(#"{"type":"DictationText","id":"d1","utterance":1,"text":"","final":true,"shaped":""}"#)!)
        _ = d.handle(event(#"{"type":"DictationText","id":"d1","utterance":3,"text":"and tests","final":false,"shaped":"and tests"}"#)!)
        #expect(d.finalText == "Ship the fix." && d.partialText == "and tests" && d.liveText == "Ship the fix. and tests")
    }
}
