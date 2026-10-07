import AppKit
import CompanionCore
import KeyboardShortcuts
import SwiftUI
import TheuthKit
import UserNotifications

/// 130 §4.6 (D12): one small native window, `Settings…` ⌘, in the menu. Values live in `SettingsModel`
/// (UserDefaults); the Store, the rail and the policy read them live.
@MainActor
final class SettingsWindowController {
    private var window: NSWindow?
    private let notifier: Notifier
    private let doubleTap: DoubleTapSource
    private let store: Store

    init(store: Store, notifier: Notifier, doubleTap: DoubleTapSource) {
        self.store = store; self.notifier = notifier; self.doubleTap = doubleTap
    }

    /// `activate`: a user asked (menu) — the window takes key. The probe shows it without activating anything.
    func show(activate: Bool) {
        if window == nil {
            let view = SettingsView(model: SettingsModel.shared, notifier: notifier, store: store, doubleTap: doubleTap)
            let w = NSWindow(contentViewController: NSHostingController(rootView: view))
            w.title = "theuth Companion"
            w.styleMask = [.titled, .closable]
            w.isReleasedWhenClosed = false
            w.setContentSize(NSSize(width: 500, height: 400))
            w.center()
            w.sharingType = Flags.shareable ? .readOnly : .none
            window = w
        }
        notifier.refreshAuthorization()
        if activate {
            NSApp.activate(ignoringOtherApps: true)
            window?.makeKeyAndOrderFront(nil)
        } else {
            window?.orderFrontRegardless()
        }
    }

    func close() { window?.orderOut(nil) }
    var frame: NSRect { window?.frame ?? .zero }
    var windowNumber: Int? { window.map(\.windowNumber) }
}

struct SettingsView: View {
    @ObservedObject var model: SettingsModel
    @ObservedObject var notifier: Notifier
    @ObservedObject var store: Store
    let doubleTap: DoubleTapSource
    @State private var axDenied = false
    /// The probe opens on the tab it is about.
    @State private var tab = Flags.probeSettings ?? (Flags.probeNotify != nil ? "notifications" : "rail")

    var body: some View {
        TabView(selection: $tab) {
            rail.tabItem { Label("Rail", systemImage: "sidebar.right") }.tag("rail")
            keys.tabItem { Label("Keys", systemImage: "keyboard") }.tag("keys")
            notifications.tabItem { Label("Notifications", systemImage: "bell") }.tag("notifications")
            engine.tabItem { Label("Engine", systemImage: "network") }.tag("engine")
        }
        .padding(16)
        .frame(width: 500, height: 400)
    }

    private var rail: some View {
        Form {
            Picker("Side", selection: $model.value.railSide) {
                Text("Right").tag("right")
                Text("Left").tag("left")
            }
            .pickerStyle(.segmented)
            Picker("Display", selection: $model.value.railDisplay) {
                Text("Main").tag("")
                ForEach(NSScreen.screens.map(\.localizedName), id: \.self) { Text($0).tag($0) }
            }
            Toggle("Collapse to a sliver when nothing needs you", isOn: $model.value.collapseWhenQuiet)
            Text("Side and display apply on the next layout; the rail never takes focus.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private var keys: some View {
        Form {
            KeyboardShortcuts.Recorder("Open the panel", name: .openPanel)
            Toggle("Double-tap ⌥ as well", isOn: Binding(
                get: { model.value.doubleTapOption },
                set: { on in
                    if on {
                        // R1-3: the one Accessibility prompt, user-initiated. Not trusted yet → the toggle stays off.
                        let ok = doubleTap.enable(prompt: true)
                        axDenied = !ok
                        model.value.doubleTapOption = ok
                    } else {
                        doubleTap.uninstall()
                        axDenied = false
                        model.value.doubleTapOption = false
                    }
                }))
            Text(axDenied
                 ? "macOS asked for Accessibility. Allow theuth Companion in System Settings › Privacy & Security › Accessibility, then turn this on again."
                 : "Needs Accessibility: a global modifier monitor is how macOS lets an app see ⌥ pressed in other apps. Nothing else is read.")
                .font(.caption).foregroundStyle(axDenied ? .orange : .secondary)
            HStack {
                Text("Undo delay")
                Slider(value: $model.value.undoDelay, in: 0...5, step: 0.5)
                Text(String(format: "%.1f s", model.value.undoDelay)).monospacedDigit().frame(width: 40, alignment: .trailing)
            }
            Text("Answers wait this long before they are sent; ⎋ undoes. 0 s sends at once.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private var notifications: some View {
        Form {
            classPicker("Questions, approvals, decisions", $model.value.notify.blocking)
            classPicker("Reviews, finished, budget at 80 %", $model.value.notify.deferrable)
            classPicker("Errors (paused, unreachable)", $model.value.notify.error)
            Toggle("Sound on errors", isOn: $model.value.notify.soundOnErrors)
            Divider()
            HStack {
                Button("Enable notifications") { notifier.requestAuthorization() }
                    .disabled(notifier.authorization == .authorized)
                Text(authText).font(.caption).foregroundStyle(.secondary)
            }
            if notifier.authorization == .notDetermined {
                Text("Banners carry the agent's own buttons — Allow · Deny, the question's options, Reply… — so you answer without opening anything. Questions arrive at once; reviews and finished work wait for a pause in your typing or an app switch. Nothing is sent until you press a button.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if let until = notifier.pausedUntil {
                Text("Paused until \(until.formatted(date: .omitted, time: .shortened)) — the rail stays live.")
                    .font(.caption).foregroundStyle(.orange)
            }
            Divider()
            Toggle("Launch at login", isOn: $model.value.launchAtLogin)
            Text("theuth registers the companion as its login item the next time it starts (written to ~/.theuth/companion.json).")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private func classPicker(_ title: String, _ sel: Binding<NotifyMode>) -> some View {
        Picker(title, selection: sel) {
            Text("Immediately").tag(NotifyMode.immediate)
            Text("At a break").tag(NotifyMode.atBreak)
            Text("Off").tag(NotifyMode.off)
        }
    }

    private var authText: String {
        switch notifier.authorization {
        case .authorized: return "on"
        case .denied: return "off in System Settings › Notifications"
        case .provisional: return "quiet (Notification Center only)"
        default: return "not asked yet"
        }
    }

    private var engine: some View {
        Form {
            LabeledContent("Engine", value: (try? EngineLocation.discover().url.absoluteString) ?? "not found")
            LabeledContent("Connection", value: "\(store.connection)")
            LabeledContent("Data dir", value: EngineLocation.dataDir().path)
            LabeledContent("Client", value: "companion")
            LabeledContent("Hub", value: "— (Ask bar wave)")
            LabeledContent("Items · desks · sessions", value: "\(store.items.count) · \(store.agents.count) · \(store.activity.count)")
        }
    }
}
