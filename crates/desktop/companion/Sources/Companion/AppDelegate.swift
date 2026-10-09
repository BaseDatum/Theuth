import AppKit
import Combine
import CompanionCore
import KeyboardShortcuts
import TheuthKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let store = Store()
    private var rail: RailController!
    private var panel: PanelController!
    private var statusItem: NSStatusItem!
    var askBar: AskBarController! // W3 (AskBar/AskBarInstall.swift)
    let legend = LegendController() // W6: the keyboard-shortcuts legend
    private var subs = Set<AnyCancellable>()
    private var probeLoggedPending = false

    func applicationDidFinishLaunching(_ note: Notification) {
        Log.write("launch bundle=\(Bundle.main.bundleIdentifier ?? "nil") probe=\(Flags.probe.map { "\($0)" } ?? "no") shareable=\(Flags.shareable)")
        setUpStatusItem()
        rail = RailController(store: store)
        panel = PanelController(store: store)
        rail.onMark = { [weak self] mark in self?.markClicked(mark) }

        // W6: the saved global chords are probed against other apps' registrations before KeyboardShortcuts
        // registers them (fallback to the default with a notice, never a dialog).
        // 149 W1: never in a probe — a second instance beside the login item shares its UserDefaults suite, and the
        // exclusive-registration probe would read the real companion's own chords as "held by another app" and persist nil.
        if !Flags.anyProbe { Hotkeys.start(SettingsModel.shared) }
        KeyboardShortcuts.onKeyUp(for: .openPanel) { [weak self] in
            guard let self, Flags.probe == nil else { return }
            self.askBar.hotkeyUp() // W3: Attention panel when something needs you, else the Ask bar
        }
        KeyboardShortcuts.onKeyUp(for: .openAskBar) { [weak self] in
            guard let self, !AskFlags.automated else { return }
            self.askBar.toggle()
        }
        installW1(rail: rail, attention: panel) // 149 W1: agents panel + hotkey, rail pills' clicks, the W1 probes (W1Install.swift)
        // 149 W2: pointAndSay.install()
        island.install() // 149 W3
        store.onLegend = { [weak self] in self?.showLegend() }
        store.onSettings = { NotificationsInstaller.shared?.settings.show(activate: true) }
        legend.onSettings = { NotificationsInstaller.shared?.settings.show(activate: true) }
        if Flags.probeLegend { legend.runProbe() }

        store.$rail.receive(on: RunLoop.main).sink { [weak self] r in MainActor.assumeIsolated { self?.updateStatusItem(r) } }.store(in: &subs)
        store.$connection.receive(on: RunLoop.main).sink { s in Log.write("connection: \(s)") }.store(in: &subs)

        // Re-discovered before every dial: at login the companion can start before the engine has written
        // `engine.json` (it would otherwise pin the 127.0.0.1:7777 fallback forever), and a restarted engine
        // listens on a new port.
        store.start(locate: {
            do {
                let loc = try EngineLocation.discover()
                Log.write("engine \(loc.url.absoluteString) (token \(loc.token.count) chars)")
                return loc
            } catch {
                Log.write("engine discovery failed: \(error) — retrying")
                return nil
            }
        })

        if let seconds = Flags.probe { runProbe(seconds) }
        installAskBar(attention: panel) // W3: panel, key-down half of the hotkey, --ask / --ask-probe
        NotificationsInstaller.install(into: self) // W2: notifications · breakpoints · Settings · menu items
    }

    func applicationDidBecomeActive(_ note: Notification) {
        Log.write("became active (key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none"), frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?"))")
    }

    /// Revision 1: a quit must exit 0 — launchd relaunches a login item that exits non-zero.
    func applicationWillTerminate(_ note: Notification) {
        Log.write("terminate")
        exit(0)
    }

    // MARK: status item (D9)

    private func setUpStatusItem() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        guard let button = statusItem.button else { return }
        button.image = Self.trayImage()
        button.imagePosition = .imageLeading
        button.target = self
        button.action = #selector(statusClicked(_:))
        button.sendAction(on: [.leftMouseUp, .rightMouseUp])
        button.setAccessibilityLabel("theuth")
    }

    /// theuth's own tray glyph (`crates/desktop/icons/tray.png` + `@2x`, the file the Tauri tray uses,
    /// `tauri.conf.json:44`), bundled as `Contents/Resources/tray{,@2x}.png`; template so the menu bar tints it.
    /// The gold dot + count live in `attributedTitle`, a separate layer the template tinting never touches.
    private static func trayImage() -> NSImage? {
        if let url = Bundle.main.url(forResource: "tray", withExtension: "png"), let img = NSImage(contentsOf: url) {
            if let url2 = Bundle.main.url(forResource: "tray@2x", withExtension: "png"), let rep = NSImageRep(contentsOf: url2),
               !img.representations.contains(where: { $0.pixelsWide == rep.pixelsWide }) {
                rep.size = img.size
                img.addRepresentation(rep)
            }
            img.isTemplate = true
            img.size = NSSize(width: 18, height: 18)
            img.accessibilityDescription = "theuth"
            return img
        }
        Log.write("tray.png missing from the bundle — falling back to an SF Symbol")
        let img = NSImage(systemSymbolName: "circle.hexagongrid", accessibilityDescription: "theuth")
        img?.isTemplate = true
        return img
    }

    private func updateStatusItem(_ r: Rail) {
        guard let button = statusItem.button else { return }
        if r.needsYouCount > 0 {
            // The gold dot and the count beside the template glyph.
            button.attributedTitle = NSAttributedString(string: " ●\(r.needsYouCount)", attributes: [
                .foregroundColor: Palette.gold,
                .font: NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .semibold),
            ])
            button.setAccessibilityValue("\(r.needsYouCount) need you")
        } else {
            button.attributedTitle = NSAttributedString(string: "")
            button.setAccessibilityValue("nothing needs you")
        }
    }

    @objc private func statusClicked(_ sender: NSStatusBarButton) {
        guard Flags.probe == nil else { return }
        let e = NSApp.currentEvent
        if e?.type == .rightMouseUp || e?.modifierFlags.contains(.option) == true {
            statusItem.menu = statusMenu()
            sender.performClick(nil)
            statusItem.menu = nil
            return
        }
        panel.toggle(anchor: .statusItem(sender.window?.frame ?? .zero))
    }

    /// The status item's menu; 149 W1 pops the same menu at the rail's "more" pill.
    func statusMenu() -> NSMenu {
        let menu = NSMenu()
        menu.addItem(withTitle: "Open theuth", action: #selector(openTheuth), keyEquivalent: "")
        let shortcuts = menu.addItem(withTitle: "Keyboard shortcuts…", action: #selector(showLegend), keyEquivalent: "/") // W6
        shortcuts.keyEquivalentModifierMask = .command
        menu.addItem(.separator())
        menu.addItem(withTitle: "Quit theuth", action: #selector(quitTheuth), keyEquivalent: "q") // W7: the whole app, one quit
        menu.addItem(withTitle: "Quit theuth Companion", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "")
        menu.items.forEach { $0.target = $0.action == #selector(openTheuth) || $0.action == #selector(showLegend) || $0.action == #selector(quitTheuth) ? self : nil }
        askBar.addMenuItem(to: menu) // W3
        NotificationsInstaller.shared?.appendMenu(menu) // W2: Pause notifications ▸ · Launch at login · Settings…
        AgentsPanelController.shared?.addMenuItem(to: menu) // 149 W1
        return menu
    }

    /// W7 (Sky, 2026-10-07 21:00): "Open theuth" shows the view the user had open last — activate the running
    /// main app without a route; launch it plainly when it is not running; the `theuth://new` link only as the
    /// last resort when the outer bundle cannot be found.
    @objc private func openTheuth() {
        if let app = MainApp.running() {
            app.unhide()
            app.activate()
            return
        }
        if let url = MainApp.bundleURL() {
            let cfg = NSWorkspace.OpenConfiguration()
            cfg.activates = true
            NSWorkspace.shared.openApplication(at: url, configuration: cfg) { _, err in
                if let err { Log.write("open theuth failed: \(err.localizedDescription)") }
            }
            return
        }
        NSWorkspace.shared.open(DeepLink.new)
    }

    /// W7: quit the main app (its quit handshake terminates this companion and the sidecar — W1S); when it is
    /// not running, quit the companion itself.
    @objc private func quitTheuth() {
        if let app = MainApp.running() {
            Log.write("quit theuth requested → terminate \(app.processIdentifier)")
            if !app.terminate() { Log.write("terminate refused; forcing"); _ = app.forceTerminate() }
            return
        }
        NSApp.terminate(nil)
    }

    /// W6: the legend under the status item; from the panel/Ask bar (⌘/) it replaces that surface.
    @objc func showLegend() {
        guard Flags.probe == nil else { return }
        panel.close(returnKey: false)
        askBar.close(returnKey: false)
        legend.toggle(anchor: statusItem.button?.window?.frame ?? .zero)
    }

    private func markClicked(_ mark: Mark) {
        guard Flags.probe == nil else { return }
        if let id = mark.itemId {
            store.focus(id: id)
            panel.open(anchor: .rail(rail.frame(of: mark)))
        } else if let s = mark.session {
            NSWorkspace.shared.open(DeepLink.session(s))
        }
    }

    // MARK: --probe N

    /// Connect, log counts and the first `PendingPrompts`, show the rail and the panel (not key) for N s, quit.
    /// Never activates, never shows a dialog, never answers.
    private func runProbe(_ seconds: TimeInterval) {
        store.onPendingPrompts = { [weak self] p in
            guard let self, !self.probeLoggedPending else { return }
            self.probeLoggedPending = true
            Log.write("probe PendingPrompts approvals=\(p.approvals.count) asks=\(p.asks.count) secrets=\(p.secrets.count) first=\(p.approvals.first.map { "\($0.tool) \($0.risk.rawValue)" } ?? p.asks.first.map { $0.questions.first?.question ?? "" } ?? "none")")
        }
        store.onRefetched = { [weak self] in
            guard let self else { return }
            Log.write("probe attention_list=\(self.store.items.count) [\(self.store.items.map { $0.kind.rawValue }.joined(separator: ","))] agents_list=\(self.store.agents.count) list_activity=\(self.store.activity.count) live=\(self.store.activity.filter { $0.activity.live }.count) rail: needsYou=\(self.store.rail.needsYouCount) working=\(self.store.rail.workingCount) marks=\(self.store.rail.marks.count) quiet=\(self.store.rail.quiet) headline=\"\(self.store.panel.headline)\"")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [self] in
            panel.showWithoutKey(anchor: .mouse)
            Log.write("probe rail frame=\(rail.panel.frame) visible=\(rail.panel.isVisible) level=\(rail.panel.level.rawValue) sharing=\(rail.panel.sharingType.rawValue); panel frame=\(panel.panel.frame) visible=\(panel.panel.isVisible) key=\(panel.panel.isKeyWindow) active=\(NSApp.isActive)")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { [self] in
            // Ground truth from the window server (top-left origin), as spike SA read it.
            let wins = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
                .filter { ($0[kCGWindowOwnerPID as String] as? Int32) == getpid() }
                .compactMap { $0[kCGWindowBounds as String] as? [String: Any] }
                .map { "x=\($0["X"] ?? 0) y=\($0["Y"] ?? 0) w=\($0["Width"] ?? 0) h=\($0["Height"] ?? 0)" }
            Log.write("probe windows (CGWindowList): \(wins)")
            Log.write("probe done after \(seconds) s — quitting (rail=\(rail.panel.frame) panel=\(panel.panel.frame) frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?"), active=\(NSApp.isActive), key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none"), panel key=\(panel.panel.isKeyWindow))")
            NSApp.terminate(nil)
        }
    }

    // MARK: W2 hooks (notification body tap · double-tap ⌥)

    /// Open the panel on an item (a notification body tap); `nil` = just the panel.
    func openPanel(on id: String?) {
        guard Flags.anyProbe == false else { return }
        if let id { store.focus(id: id) }
        panel.open(anchor: .statusItem(statusItem.button?.window?.frame ?? .zero))
    }

    func togglePanel() { panel.toggle(anchor: .mouse) }
}
