import AppKit
import Combine
import CompanionCore
import KeyboardShortcuts
import TheuthKit

extension KeyboardShortcuts.Name {
    /// `⌃⇧Space` (D6), rebindable; no double-tap ⌥ in this wave.
    static let openPanel = Self("openPanel", default: .init(.space, modifiers: [.control, .shift]))
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let store = Store()
    private var rail: RailController!
    private var panel: PanelController!
    private var statusItem: NSStatusItem!
    private var subs = Set<AnyCancellable>()
    private var probeLoggedPending = false

    func applicationDidFinishLaunching(_ note: Notification) {
        Log.write("launch bundle=\(Bundle.main.bundleIdentifier ?? "nil") probe=\(Flags.probe.map { "\($0)" } ?? "no") shareable=\(Flags.shareable)")
        setUpStatusItem()
        rail = RailController(store: store)
        panel = PanelController(store: store)
        rail.onMark = { [weak self] mark in self?.markClicked(mark) }

        KeyboardShortcuts.onKeyUp(for: .openPanel) { [weak self] in
            guard let self, Flags.probe == nil else { return }
            self.panel.toggle(anchor: .mouse)
        }

        store.$rail.receive(on: RunLoop.main).sink { [weak self] r in MainActor.assumeIsolated { self?.updateStatusItem(r) } }.store(in: &subs)
        store.$connection.receive(on: RunLoop.main).sink { s in Log.write("connection: \(s)") }.store(in: &subs)

        do {
            let loc = try EngineLocation.discover()
            Log.write("engine \(loc.url.absoluteString) (token \(loc.token.count) chars)")
            store.start(location: loc)
        } catch {
            Log.write("engine discovery failed: \(error) — will show the rail disconnected")
        }

        if let seconds = Flags.probe { runProbe(seconds) }
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
            let menu = NSMenu()
            menu.addItem(withTitle: "Open theuth", action: #selector(openTheuth), keyEquivalent: "")
            menu.addItem(.separator())
            menu.addItem(withTitle: "Quit theuth Companion", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
            menu.items.forEach { $0.target = $0.action == #selector(openTheuth) ? self : nil }
            statusItem.menu = menu
            sender.performClick(nil)
            statusItem.menu = nil
            return
        }
        panel.toggle(anchor: .statusItem(sender.window?.frame ?? .zero))
    }

    @objc private func openTheuth() { NSWorkspace.shared.open(DeepLink.new) }

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
}
