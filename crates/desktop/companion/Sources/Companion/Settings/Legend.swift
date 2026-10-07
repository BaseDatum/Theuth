import AppKit
import Combine
import CompanionCore
import SwiftUI

/// W6 item 4b: "Keyboard shortcuts…" from the menu bar (⌘/ while the panel or the Ask bar is key, `?` in the
/// panel): a small non-activating glass panel under the status item listing the table by scope. ⎋ closes.
@MainActor
final class LegendController {
    static let width: CGFloat = 360
    let panel: KeyPanel
    private let hosting: NSHostingView<LegendView>
    private var subs = Set<AnyCancellable>()
    private var keyMonitor: Any?
    private var previousApp: NSRunningApplication?
    private var anchor: NSRect = .zero
    private(set) var isOpen = false
    var onSettings: (() -> Void)?

    init() {
        let (panel, container) = GlassPanel.make(width: Self.width, height: 420)
        self.panel = panel
        hosting = NSHostingView(rootView: LegendView(model: SettingsModel.shared, onSettings: {}))
        hosting.rootView = LegendView(model: SettingsModel.shared) { [weak self] in self?.close(returnKey: false); self?.onSettings?() }
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        panel.setAccessibilityLabel("Keyboard shortcuts")
        NotificationCenter.default.publisher(for: NSWindow.didResignKeyNotification, object: panel)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.close(returnKey: false) } }
            .store(in: &subs)
        SettingsModel.shared.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { if self?.isOpen == true { self?.place() } } }
            .store(in: &subs)
    }

    /// `anchor`: the status item's button window frame, or `.zero` for the mouse's screen top-centre.
    func open(anchor: NSRect) {
        self.anchor = anchor
        previousApp = NSWorkspace.shared.frontmostApplication
        isOpen = true
        place()
        panel.ignoresMouseEvents = false
        panel.makeKeyAndOrderFront(nil)
        if keyMonitor == nil {
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
                MainActor.assumeIsolated {
                    guard let self, self.panel.isKeyWindow else { return e }
                    if e.keyCode == 53 || KeyGrammar.intent(for: Self.press(e), scope: .panel, table: SettingsModel.shared.keys) == .legend {
                        self.close(returnKey: true); return nil
                    }
                    return e
                }
            }
        }
    }

    func toggle(anchor: NSRect) { isOpen ? close(returnKey: true) : open(anchor: anchor) }

    /// Probe: on screen, not key, mouse-transparent.
    func showWithoutKey() {
        isOpen = true
        place()
        panel.ignoresMouseEvents = true
        panel.orderFrontRegardless()
    }

    func close(returnKey: Bool) {
        guard isOpen else { return }
        isOpen = false
        if let m = keyMonitor { NSEvent.removeMonitor(m); keyMonitor = nil }
        panel.orderOut(nil)
        if returnKey, let app = previousApp, app != .current { app.activate() }
        previousApp = nil
    }

    static func press(_ e: NSEvent) -> CompanionCore.KeyPress {
        CompanionCore.KeyPress(chars: e.charactersIgnoringModifiers ?? "", keyCode: e.keyCode, modifiers: Hotkeys.modifiers(e.modifierFlags))
    }

    private func place() {
        hosting.layoutSubtreeIfNeeded()
        let size = NSSize(width: Self.width, height: min(560, max(200, hosting.fittingSize.height)))
        let screen: NSScreen
        var p: NSPoint
        if anchor != .zero {
            screen = NSScreen.screens.first { $0.frame.contains(NSPoint(x: anchor.midX, y: anchor.midY)) } ?? NSScreen.main!
            p = NSPoint(x: anchor.midX - size.width / 2, y: anchor.minY - size.height - 4)
        } else {
            let m = NSEvent.mouseLocation
            screen = NSScreen.screens.first { $0.frame.contains(m) } ?? NSScreen.main!
            let v = screen.visibleFrame
            p = NSPoint(x: v.midX - size.width / 2, y: v.maxY - v.height * 0.2 - size.height)
        }
        let v = screen.visibleFrame
        p.x = min(max(v.minX + 8, p.x), v.maxX - size.width - 8)
        p.y = min(max(v.minY + 8, p.y), v.maxY - size.height - 8)
        panel.setFrame(NSRect(origin: p, size: size), display: true)
        hosting.frame = panel.contentView?.bounds ?? .zero
    }

    // MARK: --probe-legend

    /// Show the legend (not key) for 3 s, log the window number for the shell's `screencapture -l`, quit 0.
    func runProbe() {
        Log.write("probe-legend start bundle=\(Bundle.main.bundleIdentifier ?? "nil") shareable=\(Flags.shareable) rows=\(SettingsModel.shared.keys.bindings.count) unique=\(SettingsModel.shared.keys.isUniquePerScope)")
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [self] in
            showWithoutKey()
            Log.write("probe-legend shown frame=\(panel.frame) number=\(panel.windowNumber) visible=\(panel.isVisible) key=\(panel.isKeyWindow) active=\(NSApp.isActive)")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) {
            Log.write("probe-legend done active=\(NSApp.isActive) frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?") — quitting 0")
            NSApp.terminate(nil)
        }
    }
}

/// Rendered FROM the table so it is always true; the aliases and the hold-to-talk line come from the same source.
struct LegendView: View {
    @ObservedObject var model: SettingsModel
    let onSettings: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Keyboard shortcuts").font(.system(size: 13, weight: .semibold))
                Spacer()
                Hint("⎋", "close")
            }
            .padding(.horizontal, 14).padding(.top, 12).padding(.bottom, 8)
            ForEach(KeyScope.allCases, id: \.self) { scope in
                Text(scope.title.uppercased()).font(.system(size: 10, weight: .semibold)).foregroundColor(.sand).tracking(0.6)
                    .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 3)
                ForEach(model.keys.bindings(in: scope)) { b in
                    row(b.label, chord(b))
                }
                if scope == .askBar, let hold = model.keys.chord(for: .openPanel) {
                    row("Hold to talk · release to send in 3 s", "hold \(hold.symbol)")
                }
            }
            Divider().padding(.horizontal, 14).padding(.top, 8)
            VStack(alignment: .leading, spacing: 4) {
                if let n = model.hotkeyNotice { Text(n).font(.system(size: 10.5)).foregroundColor(.carnelian) }
                Text("Panel and Ask bar keys fire only while that surface is key, so they never collide with other apps — which is why the global set stays tiny.")
                    .font(.system(size: 10.5)).foregroundColor(.sand)
                HStack {
                    Button(action: onSettings) { Text("Change in Settings…  ⌘,").font(.system(size: 11.5)) }.buttonStyle(.plain).foregroundColor(.ink)
                    Spacer()
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
        }
        .frame(width: LegendController.width)
        .foregroundColor(.ink)
        .accessibilityElement(children: .contain)
    }

    private func chord(_ b: KeyBinding) -> String {
        let aliases = KeyTable.aliases.filter { $0.action == b.action }.map(\.chord.symbol)
        guard let c = b.chord else { return "unassigned" }
        return ([c.symbol] + aliases).joined(separator: " · ")
    }

    private func row(_ label: String, _ chord: String) -> some View {
        HStack {
            Text(label).font(.system(size: 12))
            Spacer()
            Text(chord).font(.system(size: 12, weight: .medium, design: .rounded)).foregroundColor(chord == "unassigned" ? .carnelian : .ink2)
        }
        .padding(.horizontal, 14).padding(.vertical, 2.5)
        .accessibilityElement(children: .combine)
    }
}
