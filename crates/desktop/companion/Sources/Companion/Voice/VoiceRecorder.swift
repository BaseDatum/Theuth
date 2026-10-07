import AVFoundation
import CompanionCore

/// Microphone → 16 kHz mono Int16 PCM (what `transcribe` demuxes as `.wav`). The permission prompt happens
/// only here, on a user's hold; probes never reach `start()` (`AskFlags.automated`).
@MainActor
final class VoiceRecorder {
    private var engine: AVAudioEngine?
    private var samples: [Int16] = []
    private let lock = NSLock()

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
        guard let outFormat = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: Double(WAV.sampleRate), channels: 1, interleaved: true),
              let converter = AVAudioConverter(from: inFormat, to: outFormat) else { return "audio format unsupported" }
        samples = []
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
            let chunk = Array(UnsafeBufferPointer(start: p, count: Int(out.frameLength)))
            self.lock.lock(); self.samples.append(contentsOf: chunk); self.lock.unlock()
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

    /// Stop and return the WAV; empty `Data` when nothing (or under 0.2 s) was captured.
    func stop() -> Data {
        tearDown()
        lock.lock(); let s = samples; samples = []; lock.unlock()
        return s.count < Int(WAV.sampleRate) / 5 ? Data() : WAV.encode(pcm16: s)
    }

    func cancel() {
        tearDown()
        lock.lock(); samples = []; lock.unlock()
    }

    private func tearDown() {
        guard let e = engine else { return }
        e.inputNode.removeTap(onBus: 0)
        e.stop()
        engine = nil
    }
}
