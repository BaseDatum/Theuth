import CompanionCore
import SwiftUI

/// Field state the controller drives (text from dictation, focus, the mic state).
@MainActor
final class AskBarState: ObservableObject {
    @Published var text = ""
    @Published var recording = false
    /// "listening…" · "transcribing…" · a failure; nil when quiet.
    @Published var status: String?
    /// The dictated text whose hold is running: any other text means the user typed, which cancels it (§4.3).
    var voiceText: String?
    @Published var focusTick = 0

    var onSubmit: ((String) -> Void)?
    var onTyped: ((String) -> Void)?

    func focusField() { focusTick += 1 }
}

/// "Ask theuth…" · mic · hints · the receipt row (mock round 2, frame 4).
struct AskBarView: View {
    @ObservedObject var store: Store
    @ObservedObject var state: AskBarState
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: "text.cursor").font(.system(size: 13)).foregroundColor(.sand)
                TextField("Ask theuth…", text: $state.text)
                    .textFieldStyle(.plain)
                    .font(.system(size: 15))
                    .focused($focused)
                    .onSubmit { state.onSubmit?(state.text) }
                    .onChange(of: state.text) { _, new in state.onTyped?(new) }
                    .onChange(of: state.focusTick) { _, _ in focused = true }
                    .onAppear { focused = true }
                    .accessibilityLabel("Ask theuth")
                    .accessibilityHint("Return sends to theuth's desk, which routes it")
                Image(systemName: state.recording ? "mic.fill" : "mic")
                    .font(.system(size: 14))
                    .foregroundColor(state.recording ? .carnelian : .ink2)
                    .accessibilityLabel(state.recording ? "Recording" : "Hold the hotkey to dictate")
            }
            .padding(.horizontal, 14).padding(.top, 13).padding(.bottom, 10)
            HStack(spacing: 12) {
                Hint("⏎", "send to theuth · it routes")
                Hint("hold ⌃⇧Space", "to talk")
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
