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
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // 149 W1 (§3.5, F3): the wide capsule's 24 pt line — finals in ink, the open partial in sand; while recording the
            // mic glyph gives way to one carnelian bar per `DictationLevel` band (D8), mirrored in the rail's actions pill.
            HStack(spacing: 12) {
                Image(systemName: "text.cursor").font(.system(size: 15)).foregroundColor(.sand)
                if state.live {
                    (Text(state.text.isEmpty && state.partial.isEmpty ? "Listening…" : state.text).foregroundColor(state.text.isEmpty ? .sand : .ink)
                        + Text(state.text.isEmpty || state.partial.isEmpty ? "" : " ") + Text(state.partial).foregroundColor(.sand))
                        .font(Type.headline.weight(.regular)).lineLimit(3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityLabel("Dictating: \(state.text) \(state.partial)")
                } else {
                    TextField("Ask theuth…", text: $state.text)
                        .textFieldStyle(.plain)
                        .font(Type.headline.weight(.regular))
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
                        .font(.system(size: 16))
                        .foregroundColor(state.recording ? .carnelian : .ink2)
                        .accessibilityLabel(state.recording ? "Listening" : "Hold the hotkey to dictate")
                }
            }
            .padding(.horizontal, 22).padding(.top, 14).padding(.bottom, 8)
            HStack(spacing: 12) {
                if let hold = settings.keys.chord(for: .openPanel)?.symbol { Hint("hold \(hold)", "to talk") }
                Text("release → sends to theuth in \(Int(store.undo.delay)) s").font(Type.caps).foregroundColor(.sand)
                Hint("⎋", store.undo.held == nil ? "keeps editing" : "undo")
                Text("theuth routes it").font(Type.caps).foregroundColor(.sand)
                Spacer()
                if let s = state.status {
                    Text(s).font(Type.caps).foregroundColor(state.recording ? .carnelian : .sand).lineLimit(1)
                }
            }
            .padding(.horizontal, 22).padding(.bottom, 12)
            if let r = store.receipt {
                ReceiptView(receipt: r, remaining: store.holdRemaining, delay: store.undo.delay, width: AskBarController.width)
            }
        }
        .frame(width: AskBarController.width)
        .foregroundColor(.ink)
        .accessibilityElement(children: .contain)
    }
}
