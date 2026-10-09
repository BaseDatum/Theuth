import CompanionCore
import SwiftUI
import TheuthKit

/// 153 §14.3 (B rows): Settings › Dictation — "Open the mic on key-down" (§8.5), "Learn from my corrections" (an engine
/// setting, read-only here), the per-app rows placeholder (wave D), and §8.7's last-dictation timings line. Companion-local
/// values in `Settings.defaults` (`ModeKeySettings`); wave C adds the any-app delivery rows.
struct DictationSettingsTab: View {
    @AppStorage(ModeKeySettings.micOnKeyDownKey, store: Settings.defaults) private var micOnKeyDown = true
    @State private var status: DictationState?

    var body: some View {
        Form {
            Toggle("Open the mic on key-down", isOn: $micOnKeyDown)
            caption("The microphone opens the moment the panel key goes down, so the first words are not lost. A tap (under \(Int(AskVoiceMachine.holdThreshold * 1000)) ms) throws the audio away and never starts a dictation — the OS mic light blinks for it. Off: the mic opens at the hold, as before.")
            Divider()
            LabeledContent("Learn from my corrections") { Text("theuth › Settings › Voice").foregroundStyle(.secondary) }
            caption("An engine setting (`voice.history.learn`, on by default): the companion reports what you actually sent after a dictation; the engine proposes a dictionary entry when the same change repeats, as a card under the Ask bar. Change it in theuth's Settings › Voice.")
            Divider()
            AppProfilesSection() // 153 D2: Dictate anywhere + Apps (`AppProfilesSettings.swift`)
            Divider()
            LabeledContent("Last dictation") { Text(lastLine).foregroundStyle(.secondary).font(Type.mono) }
                .task { status = try? await (NSApp.delegate as? AppDelegate)?.store.client.dictationStatus() }
        }
        .padding(.trailing, 8)
    }

    /// §8.7: "mic 42 ms · first words 310 ms · final 890 ms · stop → final 420 ms".
    private var lastLine: String {
        guard let s = status else { return "—" }
        guard let l = s.last else { return s.inputDevice.map { "mic \($0) · none yet" } ?? "none yet" }
        var parts: [String] = []
        if let d = l.device ?? s.inputDevice { parts.append(d) }
        if let v = l.pressToFirstPcmMs { parts.append("mic \(v) ms") }
        if let v = l.pressToFirstPartialMs { parts.append("first words \(v) ms") }
        if let v = l.pressToFirstFinalMs { parts.append("final \(v) ms") }
        if let v = l.stopToFinalMs { parts.append("stop → final \(v) ms") }
        return parts.isEmpty ? "no timings" : parts.joined(separator: " · ")
    }

    private func caption(_ s: String) -> some View {
        Text(s).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
    }
}
