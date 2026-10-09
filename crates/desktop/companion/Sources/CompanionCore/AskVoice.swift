import Foundation
import TheuthKit

/// Hold-to-dictate (D7, W6 live): the hotkey's key-down starts a press; past `holdThreshold` the microphone
/// streams to the engine's dictation; key-up stops it and the machine waits for `DictationFinished`; the text
/// stays in the field and the same 3 s hold starts. A press shorter than the threshold is a tap (the hotkey's
/// toggle). 153 §8.5: with `micOnKeyDown` the OS microphone opens at key-down (`.openMic`) and a tap discards it
/// (`.discardMic`) — no `dictation_start` for a tap, ever. §7.4: "send it" lands as `.sendNow` (no hold). Pure —
/// the app performs the effects.
public struct AskVoiceMachine: Equatable, Sendable {
    public enum State: Equatable, Sendable {
        case idle
        case pressed(since: Date)
        case recording(since: Date)
        /// `dictation_stop` sent; the closed utterance is still flushing.
        case stopping
    }

    public enum Event: Equatable, Sendable {
        case keyDown(Date)
        case keyUp(Date)
        case tick(Date)
        /// `DictationFinished`: the whole dictated text; `send` when an utterance carried "send it" (§7.4). Also
        /// accepted while `.recording`: the engine ends a dictation itself after "send it" settles (§7.3) or at its cap.
        case dictated(String, send: Bool = false)
        case failed(String)
    }

    public enum Effect: Equatable, Sendable {
        /// §8.5: open the OS microphone at key-down; chunks buffer until the dictation id exists.
        case openMic
        /// A tap after `.openMic`: tear the recorder down and drop its buffer. No dictation was started.
        case discardMic
        /// `dictation_start`; the microphone opens here too unless `.openMic` already did (the pre-153 path).
        case startRecording
        case stopDictation
        /// A short press: the caller's toggle.
        case tap
        /// Put the text in the field and start the hold.
        case setText(String)
        /// "send it": put the text in the field and commit at once — the phrase was the confirmation (§7.4).
        case sendNow(String)
        case fail(String)
    }

    /// Below this a press is a tap; at or above it the dictation starts (and, without `micOnKeyDown`, the microphone opens).
    public static let holdThreshold: TimeInterval = 0.25
    /// The longest dictation.
    public static let maxDuration: TimeInterval = 60

    public private(set) var state: State = .idle
    /// §8.5 Settings › Dictation "Open the mic on key-down" (`ModeKeySettings.micOnKeyDown`); the controller sets it before a press.
    public var micOnKeyDown = false
    /// The key-down that opened the running gesture: `DictationOpts.pressAgoMs = now − pressedAt` (§8.7).
    public private(set) var pressedAt: Date?
    private var micOpen = false

    public init() {}

    public var isRecording: Bool { if case .recording = state { return true } else { return false } }
    /// Between key-down and the hold threshold: the mic may be open, no dictation is.
    public var isPressed: Bool { if case .pressed = state { return true } else { return false } }
    public var isBusy: Bool { state != .idle }

    public mutating func handle(_ e: Event) -> [Effect] {
        switch (state, e) {
        case (.idle, .keyDown(let t)):
            state = .pressed(since: t)
            pressedAt = t
            micOpen = micOnKeyDown
            return micOpen ? [.openMic] : []
        case (.pressed(let since), .tick(let t)) where t.timeIntervalSince(since) >= Self.holdThreshold:
            state = .recording(since: t)
            return [.startRecording]
        case (.pressed, .keyUp):
            state = .idle
            defer { micOpen = false }
            return micOpen ? [.discardMic, .tap] : [.tap]
        case (.recording, .keyUp):
            state = .stopping
            return [.stopDictation]
        case (.recording(let since), .tick(let t)) where t.timeIntervalSince(since) >= Self.maxDuration:
            state = .stopping
            return [.stopDictation]
        case (.stopping, .dictated(let text, let send)), (.recording, .dictated(let text, let send)):
            state = .idle
            micOpen = false
            let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
            return t.isEmpty ? [.fail("Nothing recognized")] : send ? [.sendNow(t)] : [.setText(t)]
        case (.stopping, .failed(let why)), (.recording, .failed(let why)), (.pressed, .failed(let why)):
            state = .idle
            micOpen = false
            return [.fail(why)]
        default:
            return []
        }
    }
}

/// 149s K1: the two surface keys open two different things. "Open the panel · hold to talk" toggles the Attention
/// panel on a tap — empty state included, never the Ask bar (130-W3's "else the Ask bar" fallback is gone) — and
/// talks on a hold (the Ask bar is the dictation surface). "Open the Ask bar" toggles the Ask bar only.
public enum SurfaceKey: Equatable, Sendable {
    case panel, askBar

    public enum Action: Equatable, Sendable { case togglePanel, toggleAskBar, talk }

    /// `held`: the press reached `AskVoiceMachine.holdThreshold` (its `.startRecording`); a `.tap` is `false`.
    public func action(held: Bool) -> Action {
        switch self {
        case .askBar: return .toggleAskBar
        case .panel: return held ? .talk : .togglePanel
        }
    }
}

/// The engine's streamed dictation (design 54 §8) reduced for the Ask bar: `dictation_start` → `DictationStarted`
/// → `DictationText` per utterance (append-only; `final` once) → `dictation_stop` → `DictationFinished`.
/// `liveText` = the closed utterances joined + the open partial. 153: `text` is `shaped ?? text` (§4.4), "scratch that"
/// arrives as an earlier utterance re-sent empty (§4.6), "send it" arms `send` until the finish (§7), `DictationDevice`
/// becomes `deviceHint` (§8.2), and the finished id stays as `correctionId` for `dictation_correction` at send (§6.1). Pure.
public struct AskDictation: Equatable, Sendable {
    public enum Phase: Equatable, Sendable {
        case idle
        /// `dictation_start` in flight.
        case starting
        case live(id: String)
        /// `dictation_stop` sent for this id.
        case stopping(id: String)
    }

    public struct Utterance: Equatable, Sendable {
        public var text: String
        public var final: Bool
        public init(text: String, final: Bool) { self.text = text; self.final = final }
    }

    public enum Event: Equatable, Sendable {
        case start
        /// The `dictation_start` reply.
        case started(id: String)
        case startFailed(String)
        /// `text` is what the field shows (`shaped ?? text`); `command` is the piece's end command, sticky until the final.
        case text(id: String, utterance: Int, text: String, final: Bool, command: EndCommand? = nil)
        case level(id: String, levels: [Float])
        /// `DictationDevice`: `device` is the mic now open (`nil` when lost).
        case device(id: String, device: String?, state: DeviceState)
        case stop
        /// `DictationFinished`; `error` is `Error { message }`'s message.
        case finished(id: String, error: String?)
        /// The user edited the field after the stop: late partials must not overwrite it.
        case typed
        /// The dictated text went out (`dictation_correction` sent) or the field was cleared: nothing left to correct.
        case settled
    }

    public private(set) var phase: Phase = .idle
    public private(set) var utterances: [Utterance] = []
    /// The last `DictationLevel` packet, 0–1, oldest first.
    public private(set) var levels: [Float] = []
    /// `already dictating` → "dictating elsewhere"; `talking` (a meeting holds the mic, 149 §3.5) → "Meeting running — use the
    /// island to ask"; a model error; a finish with `Error`.
    public private(set) var error: String?
    /// §7: a piece carried "send it"; the finish then commits without the hold. Reset by the next `.start`.
    public private(set) var send = false
    /// §8.2: "microphone lost — retrying…" / "mic: ‹name›" while this dictation runs; `nil` when quiet.
    public private(set) var deviceHint: String?
    /// §6.1: the last dictation whose text is still in a field; `dictation_correction(correctionId, sentText)` at send.
    public private(set) var correctionId: String?
    private var typedWins = false

    public init() {}

    public var isActive: Bool { phase != .idle }
    public var id: String? {
        switch phase {
        case .live(let id), .stopping(let id): return id
        default: return nil
        }
    }

    /// 149g: a `dictation_start` sent by gesture `g` belongs to the controller only while `g` is still the `current` gesture
    /// and this session still waits for it (`.starting`; an exit reset it). Otherwise its id must be stopped at once — the
    /// dictation is live on the engine with no owner — and its refusal ignored. The voice key and the Ask bar share it.
    public func awaitsStart(from g: Int, current: Int) -> Bool { g == current && phase == .starting }

    public var finalText: String { utterances.filter(\.final).map(\.text).filter { !$0.isEmpty }.joined(separator: " ") }
    public var partialText: String { utterances.last.flatMap { $0.final ? nil : $0.text } ?? "" }
    public var liveText: String { [finalText, partialText].filter { !$0.isEmpty }.joined(separator: " ") }

    /// One `Dictation*` event off the wire → the reducer's event; `nil` for `DictationStarted` (149g: it is broadcast for every
    /// dictation; ours is the `dictation_start` reply) and anything unknown. `shaped ?? text` is what lands in the field.
    public static func event(_ w: DictationWire) -> Event? {
        guard let id = w.id else { return nil }
        switch w.type {
        case "DictationText": return .text(id: id, utterance: w.utterance ?? 0, text: w.shaped ?? w.text ?? "", final: w.isFinal ?? false, command: w.command)
        case "DictationLevel": return .level(id: id, levels: w.levels ?? [])
        case "DictationDevice": return .device(id: id, device: w.device, state: w.state ?? .changed)
        case "DictationFinished": return .finished(id: id, error: w.reason?.reason == "error" ? (w.reason?.message ?? "dictation failed") : nil)
        default: return nil
        }
    }

    /// `true` when the field should take `liveText` (never after the user typed).
    public mutating func handle(_ e: Event) -> Bool {
        switch e {
        case .start:
            phase = .starting; utterances = []; levels = []; error = nil; typedWins = false
            send = false; deviceHint = nil; correctionId = nil
            return true
        case .started(let id):
            guard phase == .starting else { return false }
            phase = .live(id: id)
            return false
        case .startFailed(let why):
            phase = .idle
            error = why.contains("already dictating") ? "dictating elsewhere"
                : why.contains("talking") ? "Meeting running — use the island to ask" : why
            return false
        case .text(let id, let i, let text, let final, let command):
            guard id == self.id else { return false }
            while utterances.count <= i { utterances.append(Utterance(text: "", final: true)) }
            utterances[i] = Utterance(text: text, final: final)
            if case .send = command { send = true }
            return !typedWins
        case .level(let id, let levels):
            guard id == self.id else { return false }
            self.levels = levels
            return false
        case .device(let id, let device, let state):
            guard id == self.id else { return false }
            deviceHint = state == .lost ? "microphone lost — retrying…" : device.map { "mic: \($0)" }
            return false
        case .stop:
            if case .live(let id) = phase { phase = .stopping(id: id) }
            return false
        case .finished(let id, let err):
            guard id == self.id else { return false }
            phase = .idle; levels = []; deviceHint = nil
            if let err { error = err } else { correctionId = id }
            return !typedWins
        case .typed:
            typedWins = true
            return false
        case .settled:
            correctionId = nil
            return false
        }
    }
}
