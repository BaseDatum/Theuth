import Foundation

/// Hold-to-dictate (D7, W6 live): the hotkey's key-down starts a press; past `holdThreshold` the microphone
/// streams to the engine's dictation; key-up stops it and the machine waits for `DictationFinished`; the text
/// stays in the field and the same 3 s hold starts. A press shorter than the threshold is a tap (the hotkey's
/// toggle). Pure — the app performs the effects.
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
        /// `DictationFinished`: the whole dictated text.
        case dictated(String)
        case failed(String)
    }

    public enum Effect: Equatable, Sendable {
        case startRecording
        case stopDictation
        /// A short press: the caller's toggle.
        case tap
        /// Put the text in the field and start the hold.
        case setText(String)
        case fail(String)
    }

    /// Below this a press is a tap; at or above it the microphone opens.
    public static let holdThreshold: TimeInterval = 0.25
    /// The longest dictation.
    public static let maxDuration: TimeInterval = 60

    public private(set) var state: State = .idle

    public init() {}

    public var isRecording: Bool { if case .recording = state { return true } else { return false } }
    public var isBusy: Bool { state != .idle }

    public mutating func handle(_ e: Event) -> [Effect] {
        switch (state, e) {
        case (.idle, .keyDown(let t)):
            state = .pressed(since: t)
            return []
        case (.pressed(let since), .tick(let t)) where t.timeIntervalSince(since) >= Self.holdThreshold:
            state = .recording(since: t)
            return [.startRecording]
        case (.pressed, .keyUp):
            state = .idle
            return [.tap]
        case (.recording, .keyUp):
            state = .stopping
            return [.stopDictation]
        case (.recording(let since), .tick(let t)) where t.timeIntervalSince(since) >= Self.maxDuration:
            state = .stopping
            return [.stopDictation]
        case (.stopping, .dictated(let text)):
            state = .idle
            let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
            return t.isEmpty ? [.fail("Nothing recognized")] : [.setText(t)]
        case (.stopping, .failed(let why)), (.recording, .failed(let why)), (.pressed, .failed(let why)):
            state = .idle
            return [.fail(why)]
        default:
            return []
        }
    }
}

/// The engine's streamed dictation (design 54 §8) reduced for the Ask bar: `dictation_start` → `DictationStarted`
/// → `DictationText` per utterance (append-only; `final` once) → `dictation_stop` → `DictationFinished`.
/// `liveText` = the closed utterances joined + the open partial. Pure.
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
        case text(id: String, utterance: Int, text: String, final: Bool)
        case level(id: String, levels: [Float])
        case stop
        /// `DictationFinished`; `error` is `Error { message }`'s message.
        case finished(id: String, error: String?)
        /// The user edited the field after the stop: late partials must not overwrite it.
        case typed
    }

    public private(set) var phase: Phase = .idle
    public private(set) var utterances: [Utterance] = []
    /// The last `DictationLevel` packet, 0–1, oldest first.
    public private(set) var levels: [Float] = []
    /// `already dictating` → "dictating elsewhere"; a model error; a finish with `Error`.
    public private(set) var error: String?
    private var typedWins = false

    public init() {}

    public var isActive: Bool { phase != .idle }
    public var id: String? {
        switch phase {
        case .live(let id), .stopping(let id): return id
        default: return nil
        }
    }

    public var finalText: String { utterances.filter(\.final).map(\.text).filter { !$0.isEmpty }.joined(separator: " ") }
    public var partialText: String { utterances.last.flatMap { $0.final ? nil : $0.text } ?? "" }
    public var liveText: String { [finalText, partialText].filter { !$0.isEmpty }.joined(separator: " ") }

    /// `true` when the field should take `liveText` (never after the user typed).
    public mutating func handle(_ e: Event) -> Bool {
        switch e {
        case .start:
            phase = .starting; utterances = []; levels = []; error = nil; typedWins = false
            return true
        case .started(let id):
            guard phase == .starting else { return false }
            phase = .live(id: id)
            return false
        case .startFailed(let why):
            phase = .idle
            error = why.contains("already dictating") ? "dictating elsewhere" : why
            return false
        case .text(let id, let i, let text, let final):
            guard id == self.id else { return false }
            while utterances.count <= i { utterances.append(Utterance(text: "", final: true)) }
            utterances[i] = Utterance(text: text, final: final)
            return !typedWins
        case .level(let id, let levels):
            guard id == self.id else { return false }
            self.levels = levels
            return false
        case .stop:
            if case .live(let id) = phase { phase = .stopping(id: id) }
            return false
        case .finished(let id, let err):
            guard id == self.id else { return false }
            phase = .idle; levels = []
            if let err { error = err }
            return !typedWins
        case .typed:
            typedWins = true
            return false
        }
    }
}
