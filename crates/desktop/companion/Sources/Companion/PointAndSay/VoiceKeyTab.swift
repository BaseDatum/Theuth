import CompanionCore
import SwiftUI

/// 149 §3.8: Settings › Voice key — the chord (the table's `voiceKey` row in the Keys recorder), enabled, include the screen,
/// read what the pointer touched (the engine's Accessibility grant), annotate always / while the mouse button is held,
/// "speak the answer" (Revision 1 row 10), the 52 §8 privacy line. Values in `Settings.defaults` (`VoiceKeySettings`).
struct VoiceKeyTab: View {
    @ObservedObject var model: SettingsModel
    @AppStorage(VoiceKeySettings.enabledKey, store: Settings.defaults) private var enabled = true
    @AppStorage(VoiceKeySettings.includeScreenKey, store: Settings.defaults) private var includeScreen = true
    @AppStorage(VoiceKeySettings.accessibilityKey, store: Settings.defaults) private var accessibility = true
    @AppStorage(VoiceKeySettings.annotateMouseKey, store: Settings.defaults) private var annotateMouse = false
    @AppStorage(VoiceKeySettings.speakKey, store: Settings.defaults) private var speak = true

    var body: some View {
        Form {
            Toggle("Voice key", isOn: $enabled).onChange(of: enabled) { _, _ in PointController.hotkeyChanged() }
            KeyRow(binding: model.keys.binding(.voiceKey), model: model).disabled(!enabled)
            caption("Hold to talk, release to send; tap twice to lock hands-free, tap again to send; ⎋ cancels. The companion owns the key while it runs — the main app's own binding stands down.")
            Divider()
            Toggle("Include the screen", isOn: $includeScreen)
            Toggle("Read what the pointer touched", isOn: $accessibility).disabled(!includeScreen)
            caption("Screenshots and the accessibility read happen in the engine (its Screen Recording and Accessibility grants); the trail is drawn on the capture and the PNGs go to the desk with your words.")
            Text("Draw the trail")
            Picker("Draw the trail", selection: $annotateMouse) {
                Text("Always").tag(false)
                Text("While the mouse button is held").tag(true)
            }
            .pickerStyle(.segmented).labelsHidden()
            Divider()
            Toggle("Speak the answer", isOn: $speak)
            caption("The desk's reply is read aloud by the engine's voice and played here; ⎋ on the answer card stops it.")
        }
        .padding(.trailing, 8)
    }

    private func caption(_ s: String) -> some View {
        Text(s).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
    }
}
