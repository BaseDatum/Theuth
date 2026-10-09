import AppKit
import CompanionCore
import SwiftUI

/// 149 W3 — the D9 window stack (recon A §A5; notchprompt `OverlayWindowController.swift:12-27, 51-53, 71-90`, MIT):
/// borderless non-activating panel at `.statusBar + 1` (26, re-asserted on every show), the four collection flags,
/// never `isFloatingPanel`, `sharingType = .none` kept but best-effort (§9). Oversized while attached:
/// the pill/pane animate inside SwiftUI; `IslandHostingView.hitTest` passes clicks through outside the drawn shape.
final class IslandWindow: NSPanel {
    static let level = NSWindow.Level(rawValue: NSWindow.Level.statusBar.rawValue + 1)

    /// The strip under the pointer wants to act on the first click; the request box wants key (`sendEvent`).
    var wantsKeyOnClick: ((NSPoint) -> Bool)?
    var onDoubleClick: ((NSPoint) -> Void)?

    init() {
        super.init(contentRect: NSRect(x: 0, y: 0, width: IslandLayout.windowWidth, height: IslandLayout.windowHeight),
                   styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        level = Self.level
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = true
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        isReleasedWhenClosed = false
        isMovableByWindowBackground = false
        animationBehavior = .none
        sharingType = Flags.shareable ? .readOnly : .none
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    /// Re-assert the level and order front without touching key/main (notchprompt's lesson: something resets it).
    func show() {
        level = Self.level
        orderFrontRegardless()
    }

    /// notchprompt's trick: a non-activating panel becomes key before the mouse-down is dispatched, so SwiftUI
    /// gestures and the text field run in a key-window context — but only where the content asks (the request box).
    override func sendEvent(_ event: NSEvent) {
        if event.type == .leftMouseDown {
            let p = event.locationInWindow
            if event.clickCount == 2 { onDoubleClick?(p) }
            if !isKeyWindow, wantsKeyOnClick?(p) == true { makeKey() }
        }
        super.sendEvent(event)
    }
}

/// `acceptsFirstMouse` (notchprompt `:51-53`) + click-through outside `drawn` (recon A §A5 point 6).
final class IslandHostingView<Content: View>: NSHostingView<Content> {
    /// The island's drawn shape in this view's (AppKit, bottom-left) coordinates; nil = everything.
    var drawn: NSRect?

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let p = convert(point, from: superview)
        if let d = drawn, !d.contains(p) { return nil }
        return super.hitTest(point)
    }
}

/// Layout constants shared by the window and the views.
enum IslandLayout {
    /// The attached window is oversized so the open pane animates inside SwiftUI without a window resize (D9).
    static let windowWidth: CGFloat = 720
    static let windowHeight: CGFloat = 560
    static let openWidth: CGFloat = 560
    static let detachedWidth: CGFloat = 440
    static let stripHeight: CGFloat = 44
    /// Width added around the real notch so the closed pill can carry its text.
    static let pillWings: CGFloat = 130
}

/// Notch geometry from AppKit, not constants (recon A §A3/§A5 point 4); display identity by UUID (point 8).
enum IslandGeometry {
    struct Anchor: Equatable {
        /// The pill's rect in global screen coordinates (the notch itself, or the fallback pill under the menu bar).
        let rect: NSRect
        let notch: Bool
        let screen: NSScreen
        static func == (a: Anchor, b: Anchor) -> Bool { a.rect == b.rect && a.notch == b.notch && a.screen === b.screen }
    }

    /// The real notch: between the two auxiliary areas, `safeAreaInsets.top` tall; nil on a screen without one.
    static func notch(of s: NSScreen) -> NSRect? {
        guard s.safeAreaInsets.top > 0, let l = s.auxiliaryTopLeftArea, let r = s.auxiliaryTopRightArea, r.minX > l.maxX else { return nil }
        return NSRect(x: l.maxX, y: s.frame.maxY - s.safeAreaInsets.top, width: r.minX - l.maxX, height: s.safeAreaInsets.top)
    }

    /// The menu bar's height on `s` (frame − visibleFrame at the top); the model's 24 when it is auto-hidden.
    static func menuBarHeight(of s: NSScreen) -> CGFloat {
        let h = s.frame.maxY - s.visibleFrame.maxY
        return h > 0 ? h : IslandModel.fallbackPill.height
    }

    /// The notched screen when there is one, else the primary (the menu-bar screen, `NSScreen.screens[0]`).
    static func targetScreen(forceNoNotch: Bool) -> NSScreen? {
        if !forceNoNotch, let n = NSScreen.screens.first(where: { notch(of: $0) != nil }) { return n }
        return NSScreen.screens.first
    }

    /// The anchor on `s`: the notch, or a 300 × menu-bar pill centred under the menu bar (D9). `fakeNotch` is the
    /// probe's: a 200 × 37 notch at the top centre, so the notch pill can be seen on a Mac without one.
    static func anchor(on s: NSScreen, forceNoNotch: Bool, fakeNotch: Bool) -> Anchor {
        if !forceNoNotch, let n = notch(of: s) { return Anchor(rect: n, notch: true, screen: s) }
        if fakeNotch { return Anchor(rect: NSRect(x: s.frame.midX - 100, y: s.frame.maxY - 37, width: 200, height: 37), notch: true, screen: s) }
        let h = menuBarHeight(of: s)
        let w = IslandModel.fallbackPill.width
        return Anchor(rect: NSRect(x: s.frame.midX - w / 2, y: s.frame.maxY - h - h, width: w, height: h), notch: false, screen: s)
    }

    static func displayID(of s: NSScreen) -> CGDirectDisplayID? {
        (s.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber).map { CGDirectDisplayID($0.uint32Value) }
    }

    /// `CGDisplayCreateUUIDFromDisplayID` — stable across reboots and identical twin monitors (boring.notch's key).
    static func uuid(of s: NSScreen) -> String {
        guard let id = displayID(of: s), let u = CGDisplayCreateUUIDFromDisplayID(id)?.takeRetainedValue() else { return "display-\(s.frame)" }
        return CFUUIDCreateString(nil, u) as String
    }

    /// Screen-relative ↔ global (recon A risk e: the arrangement may change while the UUID set does not).
    static func relative(_ r: NSRect, to s: NSScreen) -> NSRect { r.offsetBy(dx: -s.frame.minX, dy: -s.frame.minY) }
    static func global(_ r: NSRect, from s: NSScreen) -> NSRect { r.offsetBy(dx: s.frame.minX, dy: s.frame.minY) }
}
