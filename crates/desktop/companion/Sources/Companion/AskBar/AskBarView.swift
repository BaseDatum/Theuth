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
            HStack(spacing: 10) {
                Image(systemName: "text.cursor").font(.system(size: 13)).foregroundColor(.sand)
                if state.live {
                    // Finals normal, the open partial muted; the field comes back when the dictation lands.
                    (Text(state.text.isEmpty && state.partial.isEmpty ? "Listening…" : state.text).foregroundColor(state.text.isEmpty ? .sand : .ink)
                        + Text(state.text.isEmpty || state.partial.isEmpty ? "" : " ") + Text(state.partial).foregroundColor(.sand))
                        .font(.system(size: 15)).lineLimit(3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityLabel("Dictating: \(state.text) \(state.partial)")
                } else {
                    TextField("Ask theuth…", text: $state.text)
                        .textFieldStyle(.plain)
                        .font(.system(size: 15))
                        .focused($focused)
                        .onChange(of: state.text) { _, new in state.onTyped?(new) }
                        .onChange(of: state.focusTick) { _, _ in focused = true }
                        .onAppear { focused = true }
                        .accessibilityLabel("Ask theuth")
                        .accessibilityHint("Return sends to theuth's desk, which routes it")
                }
                Image(systemName: state.recording ? "mic.fill" : "mic")
                    .font(.system(size: 14))
                    .foregroundColor(state.recording ? .carnelian : .ink2)
                    .scaleEffect(state.recording ? 1 + CGFloat(min(1, state.level)) * 0.5 : 1)
                    .animation(.linear(duration: 0.1), value: state.level)
                    .accessibilityLabel(state.recording ? "Listening" : "Hold the hotkey to dictate")
            }
            .padding(.horizontal, 14).padding(.top, 13).padding(.bottom, 10)
            HStack(spacing: 12) {
                Hint("⏎", "send to theuth · it routes")
                if let hold = settings.keys.chord(for: .openPanel)?.symbol { Hint("hold \(hold)", "to talk · release to send in 3 s") }
                Hint("⎋", store.undo.held == nil ? "cancel" : "undo")
                Spacer()
                if let s = state.status {
                    Text(s).font(.system(size: 10.5)).foregroundColor(state.recording ? .carnelian : .sand).lineLimit(1)
                }
            }
            .padding(.horizontal, 14).padding(.bottom, 9)
            if let r = store.receipt {
                ReceiptView(receipt: r, remaining: store.holdRemaining, delay: store.undo.delay, width: AskBarController.width)
            }
        }
        .frame(width: AskBarController.width)
        .foregroundColor(.ink)
        .accessibilityElement(children: .contain)
    }
}
