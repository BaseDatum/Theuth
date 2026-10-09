import AppKit
import CompanionCore
import TheuthKit

/// 149 §3.7 / D13: one click-through, borderless, non-activating panel per `NSScreen` at `.popUpMenu` (101), the trail and
/// the 14 pt ring stroked natively (`CAShapeLayer`, `OverlayModel.strokeWidth`, round caps, alpha 0.85, malachite). Built
/// lazily per gesture from the current screen set; the screenshot never sees them because `conceal()` orders every one
/// out and the caller awaits one run-loop turn before `screen_context` (`DESK/point/mod.rs:307-322`'s rule).
@MainActor
final class OverlayWindows {
    private var panels: [String: (panel: NSPanel, view: OverlayView)] = [:]
    private(set) var revealed = false

    /// `catchMouse`: `annotate = mouse` — the overlays take the left button so a drawn stroke never clicks through.
    func reveal(catchMouse: Bool) {
        let screens = NSScreen.screens
        let wanted = Set(screens.map(\.displayID))
        for (id, p) in panels where !wanted.contains(id) { p.panel.orderOut(nil); panels[id] = nil }
        for s in screens {
            let entry = panels[s.displayID] ?? make(for: s)
            panels[s.displayID] = entry
            entry.panel.setFrame(s.frame, display: false)
            entry.panel.ignoresMouseEvents = !catchMouse
            entry.view.clear()
            entry.panel.level = .popUpMenu
            entry.panel.orderFrontRegardless()
        }
        revealed = true
    }

    /// Every overlay off screen. Synchronous; the caller hops the run loop once before capturing (D13).
    func conceal() {
        for p in panels.values { p.panel.orderOut(nil); p.view.clear() }
        revealed = false
    }

    /// Redraw from the whole sample set (one pass per tick: a few thousand points at most, `PointerSampler.maxSamples`).
    func draw(samples: [PointerSample], displays: [OverlayDisplay]) {
        let paths = OverlayModel.paths(samples: samples, displays: displays)
        for (id, p) in panels { p.view.show(paths.filter { $0.display == id }) }
    }

    var count: Int { panels.count }
    var frames: [String: NSRect] { panels.mapValues { $0.panel.frame } }

    private func make(for screen: NSScreen) -> (NSPanel, OverlayView) {
        let panel = NSPanel(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = .popUpMenu
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        panel.sharingType = Flags.shareable ? .readOnly : .none // second, best-effort layer (D13); the order-out is the real one
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.animationBehavior = .none
        panel.ignoresMouseEvents = true
        let view = OverlayView(frame: NSRect(origin: .zero, size: screen.frame.size))
        view.autoresizingMask = [.width, .height]
        panel.contentView = view
        return (panel, view)
    }
}

/// The trail layers for one display: one `CAShapeLayer` per run plus the ring. Flipped, so `OverlayPath`'s top-left local
/// points are the layer's points as they are.
final class OverlayView: NSView {
    private var runs: [CAShapeLayer] = []
    private let ring = CAShapeLayer()

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = .clear
        ring.fillColor = nil
        ring.lineWidth = 2
        ring.opacity = Float(OverlayModel.alpha)
        ring.isHidden = true
        layer?.addSublayer(ring)
    }

    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    func clear() { show([]) }

    func show(_ paths: [OverlayPath]) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        let color = Palette.malachite.cgColor
        while runs.count > paths.count { runs.removeLast().removeFromSuperlayer() }
        while runs.count < paths.count {
            let l = CAShapeLayer()
            l.fillColor = nil
            l.lineWidth = OverlayModel.strokeWidth
            l.lineCap = .round
            l.lineJoin = .round
            l.opacity = Float(OverlayModel.alpha)
            layer?.insertSublayer(l, below: ring)
            runs.append(l)
        }
        var ringAt: CGPoint?
        for (l, p) in zip(runs, paths) {
            l.strokeColor = color
            let path = CGMutablePath()
            if let first = p.points.first {
                path.move(to: first)
                p.points.count == 1 ? path.addLine(to: first) : path.addLines(between: p.points)
            }
            l.path = path
            if let r = p.ring { ringAt = r }
        }
        if let r = ringAt {
            let d = OverlayModel.ringDiameter
            ring.strokeColor = color
            ring.path = CGPath(ellipseIn: CGRect(x: r.x - d / 2, y: r.y - d / 2, width: d, height: d), transform: nil)
            ring.isHidden = false
        } else {
            ring.isHidden = true
        }
    }
}
