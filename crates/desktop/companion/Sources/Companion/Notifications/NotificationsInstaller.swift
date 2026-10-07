import AppKit
import CompanionCore
import TheuthKit

/// W2's one hook into `AppDelegate`: builds the notifier, the breakpoint sources, the double-tap monitor and
/// the Settings window; appends the D9 menu items; runs `--probe-notify` / `--request-notifications`.
@MainActor
final class NotificationsInstaller: NSObject {
    private(set) static var shared: NotificationsInstaller?

    let notifier: Notifier
    let settings: SettingsWindowController
    let doubleTap = DoubleTapSource()
    private weak var app: AppDelegate?

    static func install(into app: AppDelegate) {
        let i = NotificationsInstaller(app: app)
        shared = i
        i.start()
    }

    private init(app: AppDelegate) {
        self.app = app
        notifier = Notifier(store: app.store, dryRun: Flags.probeNotify != nil) { [weak app] id in app?.openPanel(on: id) }
        settings = SettingsWindowController(store: app.store, notifier: notifier, doubleTap: doubleTap)
        super.init()
    }

    private func start() {
        notifier.start()
        doubleTap.onDoubleTap = { [weak self] in
            guard Flags.anyProbe == false else { return }
            self?.app?.togglePanel()
        }
        if SettingsModel.shared.value.doubleTapOption {
            // Already opted in: install silently if trusted; never prompt at launch.
            if !doubleTap.enable(prompt: false) { Log.write("double-tap ⌥: on in settings but not trusted — not installed") }
        }
        if !FileManager.default.fileExists(atPath: Settings.loginItemFile.path) { SettingsModel.shared.writeLoginItem() }
        if Flags.requestNotifications { notifier.requestAuthorization() }
        if let s = Flags.probeNotify { runProbe(s) }
    }

    // MARK: menu (D9): appended after W1's items

    /// Inserted before W1's trailing "— · Quit" pair so the order is the design's (§4.5).
    func appendMenu(_ menu: NSMenu) {
        var at = menu.items.count
        if menu.items.count >= 2, menu.items[menu.items.count - 2].isSeparatorItem { at = menu.items.count - 2 }
        let pause = NSMenuItem(title: "Pause notifications", action: nil, keyEquivalent: "")
        let sub = NSMenu()
        sub.addItem(item("1 hour", #selector(pauseHour), state: notifier.pausedUntil != nil && !isUntilTomorrow))
        sub.addItem(item("Until tomorrow", #selector(pauseTomorrow), state: isUntilTomorrow))
        sub.addItem(item("Off", #selector(resume), state: notifier.pausedUntil == nil))
        pause.submenu = sub
        let s = item("Settings…", #selector(showSettings), state: false)
        s.keyEquivalent = ","
        s.keyEquivalentModifierMask = .command
        for (i, m) in [NSMenuItem.separator(), pause, item("Launch at login", #selector(toggleLogin), state: SettingsModel.shared.value.launchAtLogin), s].enumerated() {
            menu.insertItem(m, at: at + i)
        }
    }

    private func item(_ title: String, _ sel: Selector, state: Bool) -> NSMenuItem {
        let m = NSMenuItem(title: title, action: sel, keyEquivalent: "")
        m.target = self
        m.state = state ? .on : .off
        return m
    }

    private var isUntilTomorrow: Bool {
        guard let u = notifier.pausedUntil else { return false }
        return Calendar.current.isDate(u, inSameDayAs: Notifier.tomorrow) && u.timeIntervalSinceNow > 3600
    }

    @objc private func pauseHour() { notifier.pause(until: Date().addingTimeInterval(3600)) }
    @objc private func pauseTomorrow() { notifier.pause(until: Notifier.tomorrow) }
    @objc private func resume() { notifier.pause(until: nil) }
    @objc private func toggleLogin() { SettingsModel.shared.value.launchAtLogin.toggle() }
    @objc private func showSettings() { settings.show(activate: true) }

    // MARK: --probe-notify N

    /// Dry-run the policy against the live engine for N s (nothing posted, no authorization), show Settings
    /// for the last 3 s without activating, screenshot it, quit. Log: `/tmp/130-w2-probe.log`.
    private func runProbe(_ seconds: TimeInterval) {
        try? FileManager.default.removeItem(at: Notifier.probeLog)
        notifier.probeLine("start probe-notify \(seconds) s bundle=\(Bundle.main.bundleIdentifier ?? "nil") prefs=\(SettingsModel.shared.value.notify) authorization=not requested")
        let shot = "/tmp/130-w2-settings.png"
        DispatchQueue.main.asyncAfter(deadline: .now() + max(1, seconds - 3)) { [self] in
            settings.show(activate: false)
            notifier.probeLine("settings window shown frame=\(settings.frame) active=\(NSApp.isActive) key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none")")
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [self] in
                if let n = settings.windowNumber {
                    let p = Process()
                    p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
                    p.arguments = ["-x", "-o", "-l", "\(n)", shot]
                    try? p.run(); p.waitUntilExit()
                    notifier.probeLine("screenshot \(shot) exit=\(p.terminationStatus)")
                }
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [self] in
            let s = app?.store
            notifier.probeLine("done items=\(s?.items.count ?? 0) [\(s.map { Dictionary(grouping: $0.items, by: { $0.kind.rawValue }).map { "\($0.key)×\($0.value.count)" }.sorted().joined(separator: " ") } ?? "")] classes=[\(s.map { Dictionary(grouping: $0.items, by: { NotificationPolicy.classify($0).rawValue }).map { "\($0.key)×\($0.value.count)" }.sorted().joined(separator: " ") } ?? "")] would-have-posted=\(notifier.probeCounts) flood=\(notifier.inFlood) held=\(notifier.heldCount) paused=\(notifier.pausedUntil != nil) connection=\(s.map { "\($0.connection)" } ?? "?") active=\(NSApp.isActive) frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?") — quitting")
            NSApp.terminate(nil)
        }
    }
}
