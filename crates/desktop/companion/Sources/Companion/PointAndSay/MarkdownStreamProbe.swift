import AppKit
import CompanionCore
import SwiftUI

extension Flags {
    /// 189: `--probe-markdown-stream` — replay the live 333 ask through the real answer card, log and snapshot it (`MarkdownStreamProbe`).
    static let probeMarkdownStream = args.contains("--probe-markdown-stream")
}

/// 189: a deterministic replay of the live 333 ask (2026-11-11 15:33) through the real `AnswerCard` and `AnswerCardState`
/// with the animated config. `main.swift` runs it before the app delegate: no engine connection, no hotkeys. The card is
/// ordered front (never key) at an origin left of every screen, or in the real island area with `--on-screen`.
/// Two passes: collapsed (what the live card was: it auto-hid 6 s after the finish, which only a collapsed card does) and
/// expanded (the markdown body). Each: `begin` (thinking) → 0.5 s → a `TextDelta` of the reply's first 40 characters →
/// 1.5 s → the catch-up replaces it with the full 113-character reply and finishes (what `Store.renderAsk` writes) →
/// frames at 0.3, 1, 2, 3.5 and 5 s. Per frame it logs the panel height, the hosting view's `fittingSize` height, the
/// `AnswerMarkdown` height (`onGeometryChange`, "n/a" when collapsed), whether the window is on a screen and visible,
/// and each text view: its text, `alphaValue`, lowest foreground alpha, and visible/bounds height. PNGs (the hosting
/// view over the window background; `cacheDisplay` ignores layer opacity, which the alpha lines report):
/// `/tmp/189-stream-<t>.png` (collapsed) and `/tmp/189-stream-expanded-<t>.png`. Exit 0; no pass/fail judgement.
@MainActor
enum MarkdownStreamProbe {
    static let reply = "Test received — I'm here and reading.\n\n## Next\n- `Run the Hans supervision tick now`\n- `What's still owed to me?`"
    static let frames: [Double] = [0.3, 1.0, 2.0, 3.5, 5.0]

    static func run() -> Never {
        var content: CGFloat?
        AnswerMarkdown.contentHeight = { content = $0 }
        log("reply \(reply.count) chars; delta \(String(reply.prefix(40)).debugDescription)")
        var preview = AskReply(session: "")
        preview.text = reply
        log("collapsed preview (AskReply.firstTwoLines, lineLimit 2): \(preview.firstTwoLines.debugDescription)")
        for expanded in [false, true] {
            content = nil
            pass(expanded: expanded) { content }
        }
        log("done")
        exit(0)
    }

    private static func pass(expanded: Bool, content: () -> CGFloat?) {
        let name = expanded ? "expanded" : "collapsed"
        let card = AnswerCard()
        // Off every screen by default; `--on-screen` keeps the real island-area placement (a window off every screen may
        // get no display-link callbacks, and the library's per-word fade runs on one: `ParagraphNSView.setUpDisplayLink`).
        if !Flags.args.contains("--on-screen") {
            let left = NSScreen.screens.map(\.frame.minX).min() ?? 0
            card.fixedOrigin = NSPoint(x: left - AnswerCard.width - 400, y: 200)
        }

        var s = AnswerCardState()
        s.begin(question: "this is a test", kind: .ask)
        s.status = "thinking"
        s.expanded = expanded
        card.state = s
        card.show(near: .zero, key: false)
        spin(0.5)

        s = card.state // the first TextDelta, as `renderAsk` maps it
        s.text = String(reply.prefix(40)); s.phase = .answer; s.status = ""
        card.state = s
        spin(0.2)
        snapshot(card, label: "\(name) delta+0.2", png: nil, content: content())
        spin(1.3)

        s = card.state // the catch-up: the transcript's text replaces the stream and the reply finishes
        s.text = reply; s.phase = .answer; s.status = ""; s.turnFinished = true
        card.state = s
        let t0 = Date()
        for t in frames {
            spin(until: t0.addingTimeInterval(t))
            let tag = t == t.rounded() ? "\(Int(t))" : "\(t)"
            let png = URL(fileURLWithPath: "/tmp/189-stream-\(expanded ? "expanded-" : "")\(tag).png")
            snapshot(card, label: "\(name) catch-up+\(tag)s", png: png, content: content())
        }
        card.hide(returnKey: false)
        spin(0.3)
    }

    private static func snapshot(_ card: AnswerCard, label: String, png: URL?, content: CGFloat?) {
        let panel = card.panel
        guard let root = panel.contentView else { return log("\(label): no content view") }
        let hosting = root.subviews.first { $0 is NSHostingView<AnswerCardView> }
        let onScreen = panel.screen != nil, visible = panel.occlusionState.contains(.visible)
        log("\(label): panel h \(Int(panel.frame.height)) · fitting h \(hosting.map { "\(Int($0.fittingSize.height))" } ?? "?") · markdown h \(content.map { "\(Int($0))" } ?? "n/a") · on screen \(onScreen) visible \(visible)")
        for tv in textViews(in: root) {
            var minAlpha: CGFloat = 1
            if let ts = tv.textStorage, ts.length > 0 {
                ts.enumerateAttribute(.foregroundColor, in: NSRange(location: 0, length: ts.length)) { v, _, _ in
                    if let c = v as? NSColor { minAlpha = min(minAlpha, c.alphaComponent) }
                }
            }
            let text = String(tv.string.prefix(40)).replacingOccurrences(of: "\n", with: "⏎")
            log("  text \(text.debugDescription): alpha \(fmt(tv.alphaValue)) · min fg alpha \(fmt(minAlpha)) · visible \(Int(tv.visibleRect.height))/\(Int(tv.bounds.height)) pt")
        }
        // The hosting view, not the container: `cacheDisplay` draws nothing of the glass backdrop, and label colours need
        // the appearance's own background under them. What the compositor shows (layer opacity) is in the alpha lines above.
        guard let png, let view = hosting else { return }
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return log("  no bitmap for \(view.bounds)") }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        view.effectiveAppearance.performAsCurrentDrawingAppearance { NSColor.windowBackgroundColor.setFill() }
        NSRect(origin: .zero, size: view.bounds.size).fill()
        NSGraphicsContext.restoreGraphicsState()
        view.cacheDisplay(in: view.bounds, to: rep)
        do { try rep.representation(using: .png, properties: [:])?.write(to: png); log("  → \(png.path)") }
        catch { log("  writing \(png.path) failed: \(error)") }
    }

    private static func textViews(in view: NSView) -> [NSTextView] {
        view.subviews.flatMap { v -> [NSTextView] in (v as? NSTextView).map { [$0] } ?? textViews(in: v) }
    }

    private static func spin(_ seconds: Double) { spin(until: Date().addingTimeInterval(seconds)) }

    private static func spin(until deadline: Date) {
        while Date() < deadline { RunLoop.main.run(until: min(deadline, Date().addingTimeInterval(0.02))) }
    }

    private static func fmt(_ v: CGFloat) -> String { String(format: "%.2f", Double(v)) }

    private static func log(_ s: String) { Log.write("probe-markdown-stream: \(s)") }
}
