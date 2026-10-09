import CompanionCore
import CoreGraphics
import SwiftUI

/// 153 §10 / §12.6: Settings › Command — the command key switch and its row in the Keys recorder, the gesture picker
/// (hold · double-tap locks / Automatic), "Include the screen" (off by default: the words name the target), and the opt-in
/// modifier-only chord row, enabled only once macOS grants Input Monitoring (Q1). Values in `Settings.defaults`
/// (`ModeKeySettings`, `CommandSettings`).
struct CommandSettingsTab: View {
    @ObservedObject private var model = SettingsModel.shared
    @AppStorage(ModeKeySettings.commandKeyEnabledKey, store: Settings.defaults) private var enabled = true
    @AppStorage(ModeKeySettings.gestureKey(.command), store: Settings.defaults) private var gesture = ModeKeySettings.Gesture.holdLock.rawValue
    @AppStorage(CommandSettings.includeScreenKey, store: Settings.defaults) private var includeScreen = false
    @AppStorage(ModeKeySettings.modifierOnlyKeysKey, store: Settings.defaults) private var modifierOnly = false
    @State private var inputMonitoring = CGPreflightListenEventAccess()

    var body: some View {
        Form {
            Toggle("Command key", isOn: $enabled).onChange(of: enabled) { _, _ in PointController.hotkeyChanged() }
            KeyRow(binding: model.keys.binding(.command), model: model).disabled(!enabled)
            caption("Hold to speak an action for this computer (\"move today's screenshots into Desktop/Shots\"); the desk acts, verifies and answers with one ✓/✗ line in the card. Pressing it while the voice key is held switches that gesture to command mode.")
            Divider()
            GesturePicker(gesture: $gesture)
            Divider()
            Toggle("Include the screen", isOn: $includeScreen)
            caption("Off by default: a command names its target in words. On, the pointer trail and a capture go with it, as the voice key's do.")
            Divider()
            Toggle("Modifier-only chord (incl. fn)", isOn: $modifierOnly).disabled(!inputMonitoring)
            if inputMonitoring {
                caption("Set System Settings › Keyboard › Press 🌐 key to: Do Nothing, then record fn or a lone modifier in Keys.")
            } else {
                HStack {
                    caption("Needs Input Monitoring — a lone modifier cannot be a system hot key.")
                    Spacer()
                    Button("Grant Input Monitoring…") {
                        _ = CGRequestListenEventAccess() // the ONE place this prompt may appear (§16.2: never from a probe)
                        inputMonitoring = CGPreflightListenEventAccess()
                    }
                }
            }
        }
        .padding(.trailing, 8)
        .onAppear { inputMonitoring = CGPreflightListenEventAccess() }
    }

    private func caption(_ s: String) -> some View {
        Text(s).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
    }
}

/// §12.6: the per-mode gesture picker — the Voice key and Command tabs share it (`ModeKeySettings.gestureKey(mode)`).
struct GesturePicker: View {
    @Binding var gesture: String

    var body: some View {
        Text("Gesture")
        Picker("Gesture", selection: $gesture) {
            Text("Hold · double-tap locks").tag(ModeKeySettings.Gesture.holdLock.rawValue)
            Text("Automatic").tag(ModeKeySettings.Gesture.automatic.rawValue)
        }
        .pickerStyle(.segmented).labelsHidden()
        Text(gesture == ModeKeySettings.Gesture.automatic.rawValue
             ? "Tap (under 0.4 s) to start hands-free, press again to send; a longer press is push-to-talk. No double-tap lock."
             : "Hold to talk, release to send; tap twice to lock hands-free, tap again to send; ⎋ cancels.")
            .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
    }
}
