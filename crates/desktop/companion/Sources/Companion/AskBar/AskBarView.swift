import CompanionCore
import SwiftUI

/// Field state the controller drives (text from dictation, focus, the mic state).
@MainActor
final class AskBarState: ObservableObject {
    @Published var text = ""
    @Published var recording = false
    /// W6: the live dictation owns the field (finals in `text`, the open utterance in `partial`) until it lands.
    @Published var live = false
    @Published var partial = ""
    /// The last `DictationLevel` peak, 0–1: the mic glyph's pulse.
    @Published var level: Float = 0
    /// "listening…" · "finishing…" · a failure; nil when quiet.
    @Published var status: String?
    /// The dictated text whose hold is running: any other text means the user typed, which cancels it (§4.3).
    var voiceText: String?
    @Published var focusTick = 0

    var onTyped: ((String) -> Void)?

    func focusField() { focusTick += 1 }
}

/// "Ask theuth…" · mic · hints · the receipt row (mock round 2, frame 4).
struct AskBarView: View {
    @ObservedObject var store: Store
    @ObservedObject var state: AskBarState
    @ObservedObject var settings = SettingsModel.shared
    /// 149s S4: the shared speaker; the glyph shows while it speaks for this surface.
    @ObservedObject var speaker: Speaker
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // 149 W1 (§3.5, F3): the wide capsule's line — finals in primary, the open partial in tertiary; while recording the
            // mic glyph gives way to one accent bar per `DictationLevel` band (D8), mirrored in the rail's actions pill.
            HStack(spacing: 12) {
                Image(systemName: "text.cursor").font(.title3).foregroundColor(.tertiary)
                if state.live {
                    (Text(state.text.isEmpty && state.partial.isEmpty ? "Listening…" : state.text).foregroundColor(state.text.isEmpty ? .tertiary : .primary)
                        + Text(state.text.isEmpty || state.partial.isEmpty ? "" : " ") + Text(state.partial).foregroundColor(.tertiary))
                        .font(.title2).lineLimit(3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityLabel("Dictating: \(state.text) \(state.partial)")
                } else {
                    TextField("Ask theuth…", text: $state.text)
                        .textFieldStyle(.plain)
                        .font(.title2)
                        .focused($focused)
                        .onChange(of: state.text) { _, new in state.onTyped?(new) }
                        .onChange(of: state.focusTick) { _, _ in focused = true }
                        .onAppear { focused = true }
                        .accessibilityLabel("Ask theuth")
                        .accessibilityHint("Return sends to theuth's desk, which routes it")
                }
                if state.recording, !store.ask.dictation.levels.isEmpty {
                    LevelBars(levels: store.ask.dictation.levels, height: 18, barWidth: 2.5, spacing: 2)
                } else {
                    Image(systemName: state.recording ? "mic.fill" : "mic")
                        .font(.title3)
                        .foregroundColor(state.recording ? .accentColor : .secondary)
                        .accessibilityLabel(state.recording ? "Listening" : "Hold the hotkey to dictate")
                }
            }
            .padding(.horizontal, 22).padding(.top, 14).padding(.bottom, 8)
            // One line: "hold ⌃⇧Space to talk · release sends in 3 s · ⎋ keeps editing · theuth routes it".
            HStack(spacing: 8) {
                if let hold = settings.keys.chord(for: .openPanel)?.symbol { Hint("hold \(hold)", "to talk").fixedSize() }
                Text("· release sends in \(Int(store.undo.delay)) s ·").font(Type.caps).foregroundColor(.tertiary).fixedSize()
                Hint("⎋", store.undo.held == nil ? "keeps editing" : "undo").fixedSize()
                Text("· theuth routes it").font(Type.caps).foregroundColor(.tertiary).fixedSize()
                Spacer(minLength: 0)
                if speaker.surface == .askBar { SpeakingGlyph { speaker.stop() } }
                if let s = state.status {
                    Text(s).font(Type.caps).foregroundColor(state.recording ? .accentColor : .tertiary).lineLimit(1)
                }
            }
            .padding(.horizontal, 22).padding(.bottom, 12)
            if let r = store.receipt {
                ReceiptView(receipt: r, remaining: store.holdRemaining, delay: store.undo.delay, width: AskBarController.width)
            }
        }
        .frame(width: AskBarController.width)
        .foregroundColor(.primary)
        .accessibilityElement(children: .contain)
    }
}
