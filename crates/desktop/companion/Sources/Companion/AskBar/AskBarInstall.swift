import AppKit
import Combine
import CompanionCore
import KeyboardShortcuts

extension AppDelegate {
    /// W3: the Ask bar panel, the key-down half of the hotkey (key-up stays in W1's handler), `--ask`, `--ask-probe N`.
    func installAskBar(attention: PanelController) {
        askBar = AskBarController(store: store, attention: attention)
        KeyboardShortcuts.onKeyDown(for: .openPanel) { [weak self] in
            guard let self, Flags.probe == nil else { return }
            self.askBar.hotkeyDown()
        }
        if let seconds = AskFlags.probe { AskProbe.run(seconds: seconds, store: store, askBar: askBar) }
        else if AskFlags.ask { askBar.open() }
    }
}

/// `--ask-probe N --shareable`: the Ask bar on screen (not key, mouse-transparent) with "probe — do not act",
/// theuth's desk resolved through `mesh_status` + `desk_open("theuth")` (opens the desk, sends nothing), the
/// desk id and hub-ness logged to `/tmp/130-w3-probe.log`, quit after N s. Never sends, never records.
@MainActor
enum AskProbe {
    private static var subs = Set<AnyCancellable>()

    static func run(seconds: TimeInterval, store: Store, askBar: AskBarController) {
        AskLog.write("ask-probe start seconds=\(seconds) shareable=\(Flags.shareable) bundle=\(Bundle.main.bundleIdentifier ?? "nil") mic-usage-key=\(Bundle.main.object(forInfoDictionaryKey: "NSMicrophoneUsageDescription") != nil)")
        var resolved = false
        store.$connection.receive(on: RunLoop.main).sink { s in
            MainActor.assumeIsolated {
                guard s == .open, !resolved else { return }
                resolved = true
                Task {
                    do {
                        let desk = try await store.resolveDesk()
                        let hub: String
                        if case .ready(_, let h) = store.ask.router.desk { hub = "\(h)" } else { hub = "?" }
                        AskLog.write("ask-probe desk=\(desk) this-engine-is-hub=\(hub) (nothing sent)")
                    } catch {
                        AskLog.write("ask-probe desk unresolved: \(error)")
                    }
                }
            }
        }.store(in: &subs)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
            askBar.showWithoutKey(text: "probe — do not act")
            AskLog.write("ask-probe panel frame=\(askBar.panel.frame) visible=\(askBar.panel.isVisible) key=\(askBar.panel.isKeyWindow) ignoresMouse=\(askBar.panel.ignoresMouseEvents) active=\(NSApp.isActive)")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
            let wins = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
                .filter { ($0[kCGWindowOwnerPID as String] as? Int32) == getpid() }
                .compactMap { $0[kCGWindowBounds as String] as? [String: Any] }
                .map { "x=\($0["X"] ?? 0) y=\($0["Y"] ?? 0) w=\($0["Width"] ?? 0) h=\($0["Height"] ?? 0)" }
            AskLog.write("ask-probe windows (CGWindowList): \(wins)")
            AskLog.write("ask-probe done after \(seconds) s — quitting without sending (receipts=\(store.ask.receipts.count) frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?") active=\(NSApp.isActive) key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none") askbar key=\(askBar.panel.isKeyWindow))")
            NSApp.terminate(nil)
        }
    }
}
