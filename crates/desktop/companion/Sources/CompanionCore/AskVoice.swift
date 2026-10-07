import Foundation

/// Hold-to-dictate (D7): the hotkey's key-down starts a press; past `holdThreshold` it records; key-up stops
/// and transcribes; the text lands in the field and the same 3 s hold starts. A press shorter than the
/// threshold is a tap (the hotkey's toggle). Pure — the app performs the effects.
public struct AskVoiceMachine: Equatable, Sendable {
    public enum State: Equatable, Sendable {
        case idle
        case pressed(since: Date)
        case recording(since: Date)
        case transcribing
    }

    public enum Event: Equatable, Sendable {
        case keyDown(Date)
        case keyUp(Date)
        case tick(Date)
        case transcript(String)
        case failed(String)
    }

    public enum Effect: Equatable, Sendable {
        case startRecording
        case stopAndTranscribe
        /// A short press: the caller's toggle.
        case tap
        /// Put the text in the field and start the hold.
        case setText(String)
        case fail(String)
    }

    /// Below this a press is a tap; at or above it the microphone opens.
    public static let holdThreshold: TimeInterval = 0.25
    /// The longest dictation; `put_attachment` takes 16 MB, 60 s of 16 kHz PCM is 1.9 MB.
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
            state = .transcribing
            return [.stopAndTranscribe]
        case (.recording(let since), .tick(let t)) where t.timeIntervalSince(since) >= Self.maxDuration:
            state = .transcribing
            return [.stopAndTranscribe]
        case (.transcribing, .transcript(let text)):
            state = .idle
            let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
            return t.isEmpty ? [.fail("Nothing recognized")] : [.setText(t)]
        case (.transcribing, .failed(let why)):
            state = .idle
            return [.fail(why)]
        case (.recording, .failed(let why)), (.pressed, .failed(let why)):
            state = .idle
            return [.fail(why)]
        default:
            return []
        }
    }
}

/// 16 kHz mono 16-bit PCM → a RIFF/WAVE file (`put_attachment(…, "wav")` → `transcribe`).
public enum WAV {
    public static let sampleRate: UInt32 = 16_000

    public static func encode(pcm16 samples: [Int16], sampleRate: UInt32 = sampleRate) -> Data {
        let dataSize = UInt32(samples.count * 2)
        var d = Data(capacity: 44 + Int(dataSize))
        func le32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        func le16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { d.append(contentsOf: $0) } }
        d.append(contentsOf: Array("RIFF".utf8)); le32(36 + dataSize); d.append(contentsOf: Array("WAVE".utf8))
        d.append(contentsOf: Array("fmt ".utf8)); le32(16); le16(1); le16(1); le32(sampleRate); le32(sampleRate * 2); le16(2); le16(16)
        d.append(contentsOf: Array("data".utf8)); le32(dataSize)
        samples.withUnsafeBytes { d.append(contentsOf: $0) } // macOS is little-endian: Int16 bytes are already LE
        return d
    }
}
