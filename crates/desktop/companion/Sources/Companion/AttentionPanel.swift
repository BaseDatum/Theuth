import AppKit
import Combine
import CompanionCore
import SwiftUI

/// A borderless panel that can take key without activating the app (the Spotlight pattern).
final class KeyPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

/// The Attention panel (D5): 372 pt wide, ≤ 560 pt tall, glass, key only when summoned, never activates.
@MainActor
final class PanelController {
    static let width: CGFloat = 372
    static let maxHeight: CGFloat = 560

    enum Anchor {
        /// Under the status item's button window.
        case statusItem(NSRect)
        /// Beside the rail, level with a mark.
        case rail(NSRect)
        /// Top-centre of the screen with the mouse, ≈ 20 % down.
        case mouse
    }

    let panel: KeyPanel
    private let store: Store
    private let hosting: NSHostingView<PanelView>
    private var subs = Set<AnyCancellable>()
    private var keyMonitor: Any?
    private var previousApp: NSRunningApplication?
    private var anchor: Anchor = .mouse
    private(set) var isOpen = false

    init(store: Store) {
        self.store = store
        panel = KeyPanel(contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 200),
                         styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        panel.sharingType = Flags.shareable ? .readOnly : .none
        panel.becomesKeyOnlyIfNeeded = false
        panel.hidesOnDeactivate = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isReleasedWhenClosed = false
        panel.animationBehavior = .utilityWindow

        hosting = NSHostingView(rootView: PanelView(store: store))
        hosting.translatesAutoresizingMaskIntoConstraints = false

        let container = NSView()
        container.wantsLayer = true
        container.layer?.cornerRadius = 12
        container.layer?.cornerCurve = .continuous
        container.layer?.masksToBounds = true
        container.layer?.borderWidth = 0.5
        container.layer?.borderColor = Palette.border.cgColor

        let backdrop: NSView
        // Liquid Glass on 26+, the HUD material before; both honour Reduce Transparency through AppKit.
        if #available(macOS 26, *) {
            let glass = NSGlassEffectView()
            glass.cornerRadius = 12
            glass.contentView = hosting
            backdrop = glass
        } else {
            let effect = NSVisualEffectView()
            effect.material = .hudWindow
            effect.blendingMode = .behindWindow
            effect.state = .active
            effect.addSubview(hosting)
            backdrop = effect
        }
        backdrop.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(backdrop)
        NSLayoutConstraint.activate([
            backdrop.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            backdrop.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            backdrop.topAnchor.constraint(equalTo: container.topAnchor),
            backdrop.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            hosting.leadingAnchor.constraint(equalTo: backdrop.leadingAnchor),
            hosting.trailingAnchor.constraint(equalTo: backdrop.trailingAnchor),
            hosting.topAnchor.constraint(equalTo: backdrop.topAnchor),
            hosting.bottomAnchor.constraint(equalTo: backdrop.bottomAnchor),
        ])
        panel.contentView = container

        NotificationCenter.default.publisher(for: NSWindow.didResignKeyNotification, object: panel)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.close(returnKey: false) } }
            .store(in: &subs)
        store.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.relayout() } }
            .store(in: &subs)
        store.onCloseRequest = { [weak self] in self?.close(returnKey: true) }
        store.onOpenURL = { [weak self] url in
            NSWorkspace.shared.open(url)
            self?.close(returnKey: false)
        }
    }

    /// Summon: remember the frontmost app, place, take key. Never activates this app.
    func open(anchor: Anchor) {
        self.anchor = anchor
        previousApp = NSWorkspace.shared.frontmostApplication
        relayout()
        isOpen = true
        panel.makeKeyAndOrderFront(nil)
        if keyMonitor == nil {
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
                MainActor.assumeIsolated {
                    guard let self, self.panel.isKeyWindow else { return e }
                    return self.handle(e) ? nil : e
                }
            }
        }
    }

    /// Probe mode: on screen, not key, so nothing can lose focus.
    func showWithoutKey(anchor: Anchor) {
        self.anchor = anchor
        relayout()
        isOpen = true
        panel.orderFrontRegardless()
    }

    func toggle(anchor: Anchor) {
        if isOpen { close(returnKey: true) } else { open(anchor: anchor) }
    }

    func close(returnKey: Bool) {
        guard isOpen else { return }
        isOpen = false
        if let m = keyMonitor { NSEvent.removeMonitor(m); keyMonitor = nil }
        store.noteFor = nil
        panel.orderOut(nil)
        if returnKey, let app = previousApp, app != .current { app.activate() }
        previousApp = nil
    }

    /// The grammar; while the send-back note field is up only ⎋ and ⏎ are ours.
    private func handle(_ e: NSEvent) -> Bool {
        let key = KeyPress(chars: e.charactersIgnoringModifiers ?? "", keyCode: e.keyCode,
                           command: e.modifierFlags.contains(.command))
        let intent = KeyGrammar.intent(for: key)
        if store.noteFor != nil {
            guard intent == .escape else { return false }
        }
        guard intent != .none else { return false }
        store.perform(intent)
        return true
    }

    private func relayout() {
        guard isOpen || panel.isVisible else { return }
        DispatchQueue.main.async { [self] in
            hosting.layoutSubtreeIfNeeded()
            let fit = hosting.fittingSize
            let h = min(Self.maxHeight, max(80, fit.height))
            let size = NSSize(width: Self.width, height: h)
            let origin = origin(for: size)
            panel.setFrame(NSRect(origin: origin, size: size), display: true)
        }
    }

    private func origin(for size: NSSize) -> NSPoint {
        let screen: NSScreen
        var p: NSPoint
        switch anchor {
        case .statusItem(let f):
            screen = NSScreen.screens.first { $0.frame.contains(NSPoint(x: f.midX, y: f.midY)) } ?? NSScreen.main!
            p = NSPoint(x: f.midX - size.width / 2, y: f.minY - size.height - 4)
        case .rail(let f):
            screen = NSScreen.screens.first { $0.frame.contains(NSPoint(x: f.midX, y: f.midY)) } ?? NSScreen.main!
            let x = Settings.railSide == "left" ? f.maxX + 8 : f.minX - 8 - size.width
            p = NSPoint(x: x, y: f.midY - size.height / 2)
        case .mouse:
            let m = NSEvent.mouseLocation
            screen = NSScreen.screens.first { $0.frame.contains(m) } ?? NSScreen.main!
            let v = screen.visibleFrame
            p = NSPoint(x: v.midX - size.width / 2, y: v.maxY - v.height * 0.2 - size.height)
        }
        let v = screen.visibleFrame
        p.x = min(max(v.minX + 8, p.x), v.maxX - size.width - 8)
        p.y = min(max(v.minY + 8, p.y), v.maxY - size.height - 8)
        return p
    }
}
