import AppKit
import CompanionCore
import SwiftUI
import TheuthKit

/// 153 §14.3 / §9.1 (D2): Settings › Dictation › "Dictate anywhere" + "Apps" — the ⌃⌥D switch and its Keys row, the gesture
/// picker, the Accessibility status, the per-app profile rows (`ModeKeySettings.appProfiles`: terminal · polish · learn) with
/// the "Other apps" default row, and the delivery toggles (`leaveTextOnClipboard`, `checkPasteLanded`, media pause §8.6).
struct AppProfilesSection: View {
    @ObservedObject private var model = SettingsModel.shared
    @State private var enabled = AnyAppController.hotkeyEnabled
    @AppStorage(ModeKeySettings.gestureKey(.dictate), store: Settings.defaults) private var gesture = ModeKeySettings.Gesture.holdLock.rawValue
    @AppStorage(ModeKeySettings.leaveTextOnClipboardKey, store: Settings.defaults) private var leaveText = false
    @AppStorage(ModeKeySettings.checkPasteLandedKey, store: Settings.defaults) private var checkLanded = true
    @AppStorage(AnyAppController.pauseMediaKey, store: Settings.defaults) private var pauseMedia = false
    @State private var trusted = SystemState.accessibilityTrusted
    @State private var profiles = ModeKeySettings.appProfiles(Settings.defaults)

    var body: some View {
        Toggle("Dictate anywhere", isOn: $enabled).onChange(of: enabled) { _, v in
            Settings.defaults.set(v, forKey: ModeKeySettings.dictateAnywhereEnabledKey)
            PointController.hotkeyChanged()
        }
        KeyRow(binding: model.keys.binding(.dictateAnywhere), model: model).disabled(!enabled)
        caption("Hold to dictate into the app you are in: the words land as each sentence closes — written straight into text fields, pasted everywhere else (terminals, web views) with your clipboard back within half a second. Say “send it” to press Return.")
        GesturePicker(gesture: $gesture)
        HStack {
            Label(trusted ? "Accessibility granted" : "Accessibility not granted — pasting blind, no field check", systemImage: trusted ? "checkmark.circle" : "exclamationmark.triangle")
                .font(.caption).foregroundStyle(trusted ? Color.secondary : Color.orange)
            Spacer()
            if !trusted {
                Button("Open System Settings") {
                    NSWorkspace.shared.open(URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")!)
                }
                .controlSize(.small)
            }
        }
        .onAppear { trusted = SystemState.accessibilityTrusted; enabled = AnyAppController.hotkeyEnabled; PointController.hotkeyChanged() }
        Divider()
        LabeledContent("Apps") {
            Menu("Add app…") {
                ForEach(addable, id: \.bundleIdentifier) { app in
                    Button(app.localizedName ?? app.bundleIdentifier ?? "?") {
                        guard let id = app.bundleIdentifier else { return }
                        profiles.append(AppProfile(id: id, name: app.localizedName ?? id, terminal: AppProfiles.isTerminal(bundleId: id)))
                        save()
                    }
                }
            }
            .controlSize(.small).fixedSize()
        }
        ForEach($profiles) { $p in
            profileRow($p, removable: p.id != AppProfiles.anyAppId)
        }
        if !profiles.contains(where: { $0.id == AppProfiles.anyAppId }) {
            Button("Add a default for other apps") {
                profiles.append(AppProfile(id: AppProfiles.anyAppId, name: "Other apps"))
                save()
            }
            .controlSize(.small)
        }
        caption("terminal: pasted, never written into the field, shell spacing · polish: a small model tidies each sentence before it lands (theuth › Settings › Voice picks the model) · learn: report what you changed afterwards (needs Accessibility).")
        Divider()
        Toggle("Leave the dictated text on the clipboard", isOn: $leaveText)
        Toggle("Check that the text landed", isOn: $checkLanded)
        caption("Off: the clipboard is not restored after a paste. The check reads the field at 150, 350 and 1000 ms; a paste that never landed opens a card with the words.")
        Toggle("Pause media while dictating (may un-pause)", isOn: $pauseMedia)
        caption("One Play/Pause key at the press and again at the end — macOS does not say whether something is playing.")
    }

    private var addable: [NSRunningApplication] {
        let have = Set(profiles.map(\.id))
        return NSWorkspace.shared.runningApplications
            .filter { $0.activationPolicy == .regular && $0.bundleIdentifier != nil && !have.contains($0.bundleIdentifier!) && $0.bundleIdentifier != Bundle.main.bundleIdentifier }
            .sorted { ($0.localizedName ?? "") < ($1.localizedName ?? "") }
    }

    private func profileRow(_ p: Binding<AppProfile>, removable: Bool) -> some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 1) {
                Text(p.wrappedValue.id == AppProfiles.anyAppId ? "Other apps" : p.wrappedValue.name).lineLimit(1)
                if p.wrappedValue.id != AppProfiles.anyAppId { Text(p.wrappedValue.id).font(Type.mono).foregroundStyle(.tertiary).lineLimit(1) }
            }
            Spacer()
            Toggle("terminal", isOn: p.terminal).onChange(of: p.wrappedValue.terminal) { _, _ in save() }
            Toggle("polish", isOn: p.polish).onChange(of: p.wrappedValue.polish) { _, _ in save() }
            Toggle("learn", isOn: p.learn).onChange(of: p.wrappedValue.learn) { _, _ in save() }
            Button { profiles.removeAll { $0.id == p.wrappedValue.id }; save() } label: { Image(systemName: "minus.circle") }
                .buttonStyle(.borderless).disabled(!removable).accessibilityLabel("Remove \(p.wrappedValue.name)")
        }
        .toggleStyle(.checkbox).controlSize(.small)
    }

    private func save() { ModeKeySettings.setAppProfiles(profiles, Settings.defaults) }

    private func caption(_ s: String) -> some View {
        Text(s).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
    }
}
