import AVFoundation
import CompanionCore

/// Microphone → 16 kHz mono Int16 PCM in ~100 ms chunks (`onChunk`, main thread) for `dictation_audio`.
/// The permission prompt happens only here, on a user's hold; probes never reach `start()` (`AskFlags.automated`).
@MainActor
final class VoiceRecorder {
    static let sampleRate: Double = 16_000
    /// 100 ms at 16 kHz: what `dictation_audio` wants per call.
    static let chunk = 1600

    private var engine: AVAudioEngine?
    private var pending: [Int16] = []
    private let lock = NSLock()
    var onChunk: (([Int16]) -> Void)?

    /// `nil` on success, else why it did not start (shown in the Ask bar's status).
    func start() async -> String? {
        guard Bundle.main.object(forInfoDictionaryKey: "NSMicrophoneUsageDescription") != nil else {
            return "dictation needs the bundled app (no NSMicrophoneUsageDescription)"
        }
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: break
        case .notDetermined:
            guard await AVCaptureDevice.requestAccess(for: .audio) else { return "microphone access denied" }
        default: return "microphone access denied — System Settings › Privacy › Microphone"
        }
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let inFormat = input.outputFormat(forBus: 0)
        guard inFormat.sampleRate > 0, inFormat.channelCount > 0 else { return "no microphone" }
        guard let outFormat = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: Self.sampleRate, channels: 1, interleaved: true),
              let converter = AVAudioConverter(from: inFormat, to: outFormat) else { return "audio format unsupported" }
        pending = []
        input.installTap(onBus: 0, bufferSize: 4096, format: inFormat) { [weak self] buffer, _ in
            guard let self else { return }
            let ratio = outFormat.sampleRate / inFormat.sampleRate
            let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 16
            guard let out = AVAudioPCMBuffer(pcmFormat: outFormat, frameCapacity: capacity) else { return }
            var consumed = false
            var err: NSError?
            converter.convert(to: out, error: &err) { _, status in
                if consumed { status.pointee = .noDataNow; return nil }
                consumed = true
                status.pointee = .haveData
                return buffer
            }
            guard err == nil, let p = out.int16ChannelData?[0] else { return }
            // Audio thread: accumulate, hand out whole chunks on the main thread.
            self.lock.lock()
            self.pending.append(contentsOf: UnsafeBufferPointer(start: p, count: Int(out.frameLength)))
            var chunks: [[Int16]] = []
            while self.pending.count >= Self.chunk {
                chunks.append(Array(self.pending.prefix(Self.chunk)))
                self.pending.removeFirst(Self.chunk)
            }
            self.lock.unlock()
            guard !chunks.isEmpty else { return }
            Task { @MainActor in for c in chunks { self.onChunk?(c) } }
        }
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            return "microphone failed to start: \(error.localizedDescription)"
        }
        self.engine = engine
        return nil
    }

    /// Stop; the tail under one chunk goes out too.
    func stop() {
        tearDown()
        lock.lock(); let tail = pending; pending = []; lock.unlock()
        if !tail.isEmpty { onChunk?(tail) }
    }

    func cancel() {
        tearDown()
        lock.lock(); pending = []; lock.unlock()
    }

    private func tearDown() {
        guard let e = engine else { return }
        e.inputNode.removeTap(onBus: 0)
        e.stop()
        engine = nil
    }
}
