import AVFoundation
import Foundation

/// Revision 1 row 10: the spoken reply. `speak` (api.rs:5914-5932) is a CLIENT-sink utterance — one Opus packet per 80 ms
/// as `SpeechAudio` events, nothing on the engine's speaker — so the companion plays them itself: AudioToolbox's Opus
/// decoder (`kAudioFormatOpus`, through `AVAudioConverter`) → an `AVAudioPlayerNode`. The utterance is claimed by session
/// on `SpeechStarted` (the webview's `talkSession` rule: the id reply may land after the first packet).
@MainActor
final class SpeechPlayer {
    /// Opus decodes natively at 48 kHz, whatever rate it was encoded from.
    static let rate: Double = 48_000
    /// 80 ms per packet (`VoiceEvent::SpeechAudio`) at 48 kHz: the decode buffer's size.
    static let framesPerPacket: UInt32 = 3840

    private var engine: AVAudioEngine?
    private var player: AVAudioPlayerNode?
    private var converter: AVAudioConverter?
    private let pcm = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 1)!
    private var expecting: String?
    private(set) var current: String?
    private var pending = 0
    private var finished = false
    var onEnded: ((String) -> Void)?

    var isPlaying: Bool { current != nil }

    /// The next `SpeechStarted` on `session` with `sink == client` is ours.
    func expect(session: String) { expecting = session }

    func started(id: String, session: String, sink: String?) {
        guard expecting == session, sink ?? "client" == "client" else { return }
        expecting = nil
        stop()
        guard let opus = Self.opusFormat(), let c = AVAudioConverter(from: opus, to: pcm) else { return }
        let e = AVAudioEngine()
        let p = AVAudioPlayerNode()
        e.attach(p)
        e.connect(p, to: e.mainMixerNode, format: pcm)
        do { try e.start() } catch { return Log.write("speech: engine start failed: \(error)") }
        p.play()
        engine = e; player = p; converter = c; current = id; pending = 0; finished = false
    }

    func audio(id: String, opusBase64: String) {
        guard id == current, let data = Data(base64Encoded: opusBase64), let buf = decode(data), let p = player else { return }
        pending += 1
        p.scheduleBuffer(buf) { [weak self] in
            Task { @MainActor in
                guard let self else { return }
                self.pending -= 1
                if self.pending == 0, self.finished { self.end(id) }
            }
        }
    }

    /// `SpeechFinished`: the tail still plays; the end is the last buffer's callback.
    func finish(id: String, ok: Bool) {
        guard id == current else { return }
        finished = true
        if !ok || pending == 0 { end(id) }
    }

    func stop() {
        expecting = nil
        guard let id = current else { return }
        current = nil
        tearDown()
        onEnded?(id)
    }

    private func end(_ id: String) {
        guard id == current else { return }
        current = nil
        tearDown()
        onEnded?(id)
    }

    private func tearDown() {
        player?.stop()
        engine?.stop()
        player = nil; engine = nil; converter = nil
    }

    /// One packet → PCM (`AVAudioCompressedBuffer` with its packet description).
    func decode(_ packet: Data) -> AVAudioPCMBuffer? {
        guard let converter else { return nil }
        let opus = converter.inputFormat
        let inBuf = AVAudioCompressedBuffer(format: opus, packetCapacity: 1, maximumPacketSize: packet.count)
        inBuf.byteLength = UInt32(packet.count)
        inBuf.packetCount = 1
        packet.withUnsafeBytes { inBuf.data.copyMemory(from: $0.baseAddress!, byteCount: packet.count) }
        inBuf.packetDescriptions?.pointee = AudioStreamPacketDescription(mStartOffset: 0, mVariableFramesInPacket: 0, mDataByteSize: UInt32(packet.count))
        guard let out = AVAudioPCMBuffer(pcmFormat: pcm, frameCapacity: Self.framesPerPacket * 2) else { return nil }
        var fed = false
        var err: NSError?
        let status = converter.convert(to: out, error: &err) { _, outStatus in
            if fed { outStatus.pointee = .noDataNow; return nil }
            fed = true
            outStatus.pointee = .haveData
            return inBuf
        }
        guard err == nil, status != .error, out.frameLength > 0 else { return nil }
        return out
    }

    /// Opus as AudioToolbox's decoder wants it: 48 kHz mono, `mFramesPerPacket` 0 (variable). A fixed 3840 answers NO converter
    /// (checked with real libopus packets: 13 × 80 ms → 49 800 frames, rms 0.347 of a 0.354 tone, `/tmp/149-W2-opus.swift`).
    static func opusFormat() -> AVAudioFormat? {
        var asbd = AudioStreamBasicDescription(mSampleRate: rate, mFormatID: kAudioFormatOpus, mFormatFlags: 0, mBytesPerPacket: 0,
                                               mFramesPerPacket: 0, mBytesPerFrame: 0, mChannelsPerFrame: 1, mBitsPerChannel: 0, mReserved: 0)
        return AVAudioFormat(streamDescription: &asbd)
    }
}
