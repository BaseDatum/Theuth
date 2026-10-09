import AppKit
import AVFoundation
import Combine
import CompanionCore
import SwiftUI
import TheuthKit

/// 153 §6.4 (mock frame D): "Dictionary · N times this week — You changed ‹heard› → ‹typed› after dictating. Add it as a
/// replacement?" with Add / Not now / Never for this and ▶ play what you said. Fed by `store.ask.suggestion`.
struct DictionarySuggestionView: View {
    let suggestion: DictionarySuggestion
    let answer: (SuggestionAnswer) -> Void
    let play: () -> Void

    static let width: CGFloat = 420

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Dictionary").font(.subheadline.weight(.semibold))
                Spacer()
                Text("\(suggestion.occurrences) times this week").font(Type.caps).foregroundColor(.tertiary)
            }
            (Text("You changed ").foregroundColor(.secondary)
                + Text("“\(suggestion.heard)”").strikethrough().foregroundColor(.tertiary)
                + Text(" → ").foregroundColor(.secondary)
                + Text(suggestion.typed).font(.callout.monospaced()).foregroundColor(.primary)
                + Text(" after dictating.").foregroundColor(.secondary))
                .font(.callout).fixedSize(horizontal: false, vertical: true)
            Text("Heard the same way \(suggestion.occurrences == 2 ? "twice" : "\(suggestion.occurrences) times") (\(surfaceName), 7 days). Add it as a replacement?")
                .font(.caption).foregroundColor(.tertiary).fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Button("Add") { answer(.add) }
                Button("Not now") { answer(.notNow) }
                Button("Never for this") { answer(.never) }
                Spacer()
                if suggestion.hasAudio {
                    Button(action: play) { Label("play what you said", systemImage: "play.fill").font(Type.caps) }
                        .buttonStyle(.plain).foregroundColor(.secondary)
                        .accessibilityLabel("Play what you said")
                }
            }
            .controlSize(.small)
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .frame(width: Self.width)
        .accessibilityElement(children: .contain)
    }

    private var surfaceName: String {
        switch suggestion.surface {
        case .composer: return "Composer"
        case .tui: return "terminal"
        case .companion: return "companion"
        case .phone: return "phone"
        case .anyApp: return "an app"
        }
    }
}

/// The card's glass panel under the Ask bar (`AskBarController.anchorFrame`), not key, mouse-enabled. Shows whatever
/// `store.ask.suggestion` holds; hides on an answer or after `dismissAfter` (the engine counts an unanswered card itself —
/// no `not_now`, no cooldown, §6.4). ▶ fetches `dictation_audio_get(id, nil)` (A3: the whole dictation) and plays the PCM.
@MainActor
final class DictionarySuggestionCard {
    static let dismissAfter: TimeInterval = 20

    private let store: Store
    private unowned let askBar: AskBarController
    private var panel: KeyPanel?
    private var hosting: NSHostingView<DictionarySuggestionView>?
    private var dismiss: Timer?
    private var subs = Set<AnyCancellable>()
    private let player = PCMPlayer()

    init(store: Store, askBar: AskBarController) {
        self.store = store
        self.askBar = askBar
        store.$ask.map(\.suggestion).removeDuplicates().receive(on: RunLoop.main)
            .sink { [weak self] s in MainActor.assumeIsolated { self?.show(s) } }.store(in: &subs)
    }

    var isVisible: Bool { panel?.isVisible ?? false }

    private func show(_ s: DictionarySuggestion?) {
        dismiss?.invalidate(); dismiss = nil
        guard let s else { return hide() }
        let view = DictionarySuggestionView(suggestion: s,
                                            answer: { [weak self] a in self?.store.answerSuggestion(a) },
                                            play: { [weak self] in self?.play(s.id) })
        if panel == nil {
            let (p, container) = GlassPanel.make(width: DictionarySuggestionView.width, height: 120, radius: Type.cardRadius)
            p.becomesKeyOnlyIfNeeded = true // a click on a button never takes key from the Ask bar (which closes on resign)
            let h = NSHostingView(rootView: view)
            h.frame = container.bounds
            h.autoresizingMask = [.width, .height]
            container.addSubview(h)
            panel = p; hosting = h
        } else {
            hosting?.rootView = view
        }
        guard let panel, let hosting else { return }
        hosting.layoutSubtreeIfNeeded()
        let size = NSSize(width: DictionarySuggestionView.width, height: max(96, hosting.fittingSize.height))
        let a = askBar.anchorFrame
        panel.setFrame(NSRect(x: a.midX - size.width / 2, y: a.minY - 8 - size.height, width: size.width, height: size.height), display: true)
        hosting.frame = panel.contentView?.bounds ?? .zero
        panel.ignoresMouseEvents = false
        panel.orderFrontRegardless()
        AskLog.write("suggestion card shown \(s.id) frame=\(panel.frame) number=\(panel.windowNumber)")
        dismiss = Timer.scheduledTimer(withTimeInterval: Self.dismissAfter, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.store.answerSuggestion(nil) }
        }
    }

    private func hide() {
        player.stop()
        panel?.orderOut(nil)
    }

    private func play(_ id: String) {
        Task { [store, player] in
            do {
                let b64 = try await store.client.dictationAudioGet(id: id, utterance: nil)
                guard let pcm = Data(base64Encoded: b64) else { return Log.write("dictation_audio_get: bad base64") }
                player.play(pcm16: pcm)
            } catch {
                Log.write("dictation_audio_get failed: \(error)")
            }
        }
    }
}

/// 16 kHz mono i16 PCM → the speakers, the `SpeechPlayer` way (`AVAudioEngine` + `AVAudioPlayerNode`); one clip at a time.
@MainActor
final class PCMPlayer {
    static let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: VoiceRecorder.sampleRate, channels: 1, interleaved: true)!
    private var engine: AVAudioEngine?
    private var player: AVAudioPlayerNode?

    func play(pcm16: Data) {
        stop()
        let frames = AVAudioFrameCount(pcm16.count / 2)
        guard frames > 0, let buf = AVAudioPCMBuffer(pcmFormat: Self.format, frameCapacity: frames) else { return }
        buf.frameLength = frames
        pcm16.withUnsafeBytes { raw in
            guard let dst = buf.int16ChannelData?[0], let src = raw.baseAddress else { return }
            memcpy(dst, src, Int(frames) * 2)
        }
        let e = AVAudioEngine()
        let p = AVAudioPlayerNode()
        e.attach(p)
        e.connect(p, to: e.mainMixerNode, format: Self.format)
        do { try e.start() } catch { return Log.write("pcm play: engine start failed: \(error)") }
        p.scheduleBuffer(buf) { [weak self] in Task { @MainActor in self?.stop() } }
        p.play()
        engine = e; player = p
    }

    func stop() {
        player?.stop(); engine?.stop()
        player = nil; engine = nil
    }
}
