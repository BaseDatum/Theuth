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
        if let tab = Flags.probeSettings { runSettingsProbe(tab) }
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

    /// Dry-run the policy against the live engine for N s (nothing posted, no authorization), replay the live
    /// items through a fresh policy so the log shows every class, the batch and the cap even on a quiet engine,
    /// show Settings for the last 3 s without activating, quit. The screenshot is the shell's job
    /// (`screencapture` from inside the app would need Screen Recording). Log: `/tmp/130-w2-probe.log`.
    private func runProbe(_ seconds: TimeInterval) {
        try? FileManager.default.removeItem(at: Notifier.probeLog)
        notifier.probeLine("start probe-notify \(seconds) s bundle=\(Bundle.main.bundleIdentifier ?? "nil") prefs=\(SettingsModel.shared.value.notify) authorization=not requested")
        DispatchQueue.main.asyncAfter(deadline: .now() + max(1, seconds - 3)) { [self] in
            settings.show(activate: false)
            notifier.probeLine("settings window shown frame=\(settings.frame) number=\(settings.windowNumber ?? 0) active=\(NSApp.isActive) key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none")")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [self] in
            let s = app?.store
            func count<T>(_ xs: [T], by key: (T) -> String) -> String {
                Dictionary(grouping: xs, by: key).map { "\($0.key)×\($0.value.count)" }.sorted().joined(separator: " ")
            }
            notifier.probeLine("live items=\(s?.items.count ?? 0) [\(count(s?.items ?? [], by: { $0.kind.rawValue }))] classes=[\(count(s?.items ?? [], by: { NotificationPolicy.classify($0).rawValue }))] new-while-running→would-have-posted=\(notifier.probeCounts) flood=\(notifier.inFlood) held=\(notifier.heldCount) paused=\(notifier.pausedUntil != nil) connection=\(s.map { "\($0.connection)" } ?? "?")")
            replay(s?.items ?? [])
            notifier.probeLine("done after \(seconds) s active=\(NSApp.isActive) key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none") frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?") — quitting")
            NSApp.terminate(nil)
        }
    }

    // MARK: --probe-settings <tab> (W6)

    /// Show Settings on `tab` without activating for 3 s, log the window number (the shell screenshots it with
    /// `screencapture -l`), quit 0. Item 1's crash reproduces here when the KeyboardShortcuts resource bundle
    /// cannot be found.
    private func runSettingsProbe(_ tab: String) {
        Log.write("probe-settings start tab=\(tab) bundle=\(Bundle.main.bundleIdentifier ?? "nil") shareable=\(Flags.shareable)")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [self] in
            settings.show(activate: false)
            Log.write("probe-settings window shown tab=\(tab) frame=\(settings.frame) number=\(settings.windowNumber ?? 0) active=\(NSApp.isActive) key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none")")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) {
            Log.write("probe-settings done tab=\(tab) active=\(NSApp.isActive) frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?") — quitting 0")
            NSApp.terminate(nil)
        }
    }

    /// The live items as if they had just arrived: once as a whole (flood when > 10), then the first ≤ 10
    /// one at a time (per-class verdicts, the held batch at a breakpoint, the 2-minute cap, withdrawal).
    private func replay(_ items: [AttentionItem]) {
        let t0 = Date()
        let prefs = SettingsModel.shared.value.notify
        func log(_ label: String, _ out: PolicyOutput) {
            notifier.probeLine("replay \(label): post=[\(out.post.map { "\($0.category.rawValue):\($0.id) \($0.level.rawValue)\($0.sound ? " sound" : "")" }.joined(separator: ", "))] withdraw=\(out.withdraw)")
        }
        var whole = NotificationPolicy(prefs: prefs)
        _ = whole.update(items: [], now: t0)
        log("all \(items.count) at once", whole.update(items: items, now: t0))
        var one = NotificationPolicy(prefs: prefs)
        _ = one.update(items: [], now: t0)
        let few = Array(items.prefix(NotificationPolicy.floodThreshold))
        for (i, item) in few.enumerated() {
            log("item \(i + 1) \(item.kind.rawValue) (\(NotificationPolicy.classify(item).rawValue))",
                one.update(items: Array(few.prefix(i + 1)), now: t0.addingTimeInterval(Double(i))))
        }
        log("tick +60 s (cap not elapsed, held=\(one.heldCount))", one.tick(now: t0.addingTimeInterval(60)))
        log("breakpoint +61 s (held=\(one.heldCount))", one.breakpoint(now: t0.addingTimeInterval(61)))
        _ = one.update(items: Array(few.prefix(1)), now: t0.addingTimeInterval(65))
        log("items re-added +70 s (held=\(one.heldCount))", one.update(items: few, now: t0.addingTimeInterval(70)))
        log("breakpoint +80 s inside the 2-min rate window (held=\(one.heldCount))", one.breakpoint(now: t0.addingTimeInterval(80)))
        log("tick +181 s (rate window passed)", one.tick(now: t0.addingTimeInterval(181)))
        log("answered elsewhere (all gone)", one.update(items: [], now: t0.addingTimeInterval(182)))
    }
}
