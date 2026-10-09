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
    let store: Store // 149 W1: `PanelController+W1` (PanelProbe.swift) reads it
    /// 149 W1 (§3.3): the card's reply text, P's attachment and V's hold-to-talk.
    let w1: CardState
    private let hosting: NSHostingView<PanelView>
    private var subs = Set<AnyCancellable>()
    private var keyMonitor: Any?
    private var previousApp: NSRunningApplication?
    private var anchor: Anchor = .mouse
    private(set) var isOpen = false
    /// 155 §13.5: the focus set last sent to the engine; `nil` = never on this connection.
    private var sentFocus: [String]?

    init(store: Store) {
        self.store = store
        w1 = CardState(store: store)
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

        hosting = NSHostingView(rootView: PanelView(store: store, w1: w1))
        // The window frame is set by `relayout` (≤ 560 pt): autoresizing only, no constraints that could grow it.
        hosting.autoresizingMask = [.width, .height]

        let container = NSView(frame: NSRect(x: 0, y: 0, width: Self.width, height: 200))
        container.wantsLayer = true
        container.layer?.cornerRadius = Type.cardRadius // 149n: native card radius 12
        container.layer?.cornerCurve = .continuous
        container.layer?.masksToBounds = true
        container.layer?.borderWidth = 0.5
        container.layer?.borderColor = NSColor.separatorColor.cgColor

        let backdrop: NSView
        // Liquid Glass on 26+, the HUD material before; both honour Reduce Transparency through AppKit. The
        // backdrop is a sibling under the hosting view (not its `contentView`), so no autolayout constraint can
        // grow the window past `relayout`'s frame.
        if #available(macOS 26, *) {
            let glass = NSGlassEffectView()
            glass.cornerRadius = Type.cardRadius
            backdrop = glass
        } else {
            let effect = NSVisualEffectView()
            effect.material = .hudWindow
            effect.blendingMode = .behindWindow
            effect.state = .active
            backdrop = effect
        }
        backdrop.frame = container.bounds
        backdrop.autoresizingMask = [.width, .height]
        hosting.frame = container.bounds
        container.addSubview(backdrop)
        container.addSubview(hosting)
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
        // 149 W1: the reply row and the hold-to-talk change the card's height too.
        w1.objectWillChange.merge(with: w1.voice.objectWillChange).receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.relayout() } }
            .store(in: &subs)
        w1.onPoint = { [weak self] in self?.pointAndAttach() }
        w1.onStop = { [weak self] in self?.stopAgent() }
        // 155 §13.5: the card's session is "in view" while the panel shows it; a reconnect starts a fresh connection.
        store.$panel.receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.syncFocus() } }
            .store(in: &subs)
        store.$connection.receive(on: RunLoop.main)
            .sink { [weak self] s in MainActor.assumeIsolated { if s == .open { self?.sentFocus = nil; self?.syncFocus() } } }
            .store(in: &subs)
    }

    /// 155 §13.5 (D6 "never for the session in view"): `set_event_focus([session])` for the card on screen, `[]` when
    /// the panel is closed — the engine blanks that session's alerts for this connection and marks its finish seen.
    private func syncFocus() {
        let want = isOpen ? (store.focusedCard?.item.session).map { [$0] } ?? [] : []
        guard want != sentFocus else { return }
        sentFocus = want
        Task { try? await store.client.setEventFocus(sessions: want) } // -32601 on an engine before 129: ignored
    }

    /// Summon: remember the frontmost app, place, take key. Never activates this app.
    func open(anchor: Anchor) {
        self.anchor = anchor
        previousApp = NSWorkspace.shared.frontmostApplication
        isOpen = true
        syncFocus()
        relayout(now: true)
        panel.ignoresMouseEvents = false
        panel.makeKeyAndOrderFront(nil)
        panel.makeFirstResponder(nil) // 149 W1: the reply field is always there; R/space/a click focuses it, not the key window
        if keyMonitor == nil {
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp]) { [weak self] e in // 149 W1: key-up releases V
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
        isOpen = true
        relayout(now: true)
        panel.ignoresMouseEvents = true // a stray click must not make it key during the probe
        panel.orderFrontRegardless()
    }

    func toggle(anchor: Anchor) {
        if isOpen { close(returnKey: true) } else { open(anchor: anchor) }
    }

    func close(returnKey: Bool) {
        guard isOpen else { return }
        isOpen = false
        syncFocus()
        if let m = keyMonitor { NSEvent.removeMonitor(m); keyMonitor = nil }
        store.noteFor = nil
        store.ask.replyFor = nil // W3
        w1.voice.cancel() // 149 W1: a hold dies with the panel; P's attachment too
        w1.attached = nil
        panel.orderOut(nil)
        if returnKey, let app = previousApp, app != .current { app.activate() }
        previousApp = nil
    }

    /// The grammar (W6: the table, panel scope); while the send-back note field or the R field is up only ⎋ is ours.
    /// 149 W1: V/P/X (`.talk/.point/.stopAgent`, no intent) resolve through the table first; key-ups only matter to V.
    private func handle(_ e: NSEvent) -> Bool {
        if let a = SettingsModel.shared.keys.action(for: LegendController.press(e).chord, in: .panel), handleW1(a, e) { return true }
        guard e.type == .keyDown else { return false }
        let intent = KeyGrammar.intent(for: LegendController.press(e), scope: .panel, table: SettingsModel.shared.keys,
                                       textFieldFocused: store.noteFor != nil || store.ask.replyFor != nil)
        guard intent != .none else { return false }
        store.perform(intent)
        return true
    }

    /// Size to the SwiftUI content (≤ 560 pt) and place it. `objectWillChange` fires before the change, so that
    /// path defers one run-loop turn; opening measures at once.
    private func relayout(now: Bool = false) {
        guard isOpen else { return }
        if now { return place() }
        DispatchQueue.main.async { [self] in if isOpen { place() } }
    }

    private func place() {
        hosting.layoutSubtreeIfNeeded()
        let fit = hosting.fittingSize
        let size = NSSize(width: Self.width, height: min(Self.maxHeight, max(80, fit.height)))
        panel.setFrame(NSRect(origin: origin(for: size), size: size), display: true)
        hosting.frame = panel.contentView?.bounds ?? .zero
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
