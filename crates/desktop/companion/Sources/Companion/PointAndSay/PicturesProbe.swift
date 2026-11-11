import AppKit
import CompanionCore
import TheuthKit

extension Flags {
    /// 192 P6: `--probe-pictures` — picture fences through the real card against a stub `fence_snapshot` (`PicturesProbe`).
    static let probePictures = args.contains("--probe-pictures")
    /// 192 P6: `--probe-thinking` — streamed reasoning through the real card, expanded (Ask) and collapsed (voice key).
    static let probeThinking = args.contains("--probe-thinking")
}

/// 192 P6: release-Mac probes in the `--probe-markdown-stream` mould (`main.swift` runs them before the app delegate: no
/// engine connection, no hotkeys; the card is ordered front off every screen, or in the island area with `--on-screen`).
/// Per frame: the card's heights and text views (`MarkdownStreamProbe.snapshot`) and a PNG. Exit 0, no verdict.
@MainActor
enum PicturesProbe {
    /// Two closed picture fences and a third still streaming; the third closes at the finish.
    static let head = """
    Here is the week:

    ```json-render
    {"root": {"type": "Card", "title": "Spend this week", "children": []}}
    ```

    And the flow:

    ```mermaid
    graph TD
      A[ask] --> B[desk]
    ```

    Next, the plan:

    ```mermaid
    graph LR
      X --> Y
    """

    /// The stub `fence_snapshot`: 300 ms, then a fixed 408 × 180 px PNG for json-render and `failed` for anything else
    /// (deterministic by lang, whatever order the two in-flight requests start in).
    static func stubDraw(_ k: FenceKey) async throws -> FenceSnapshotReply {
        try await Task.sleep(for: .milliseconds(300))
        guard k.lang == "json-render" else { return FenceSnapshotReply(outcome: "failed", error: "stub: no picture for \(k.lang)") }
        let png = await MainActor.run { stubPNG(width: 408, height: 180, scale: k.scale, dark: k.appearance == "dark") }
        return FenceSnapshotReply(outcome: "done", png: png.base64EncodedString(), width: 408, height: 180)
    }

    static func stubPNG(width: Int, height: Int, scale: Double, dark: Bool) -> Data {
        let px = NSSize(width: Double(width) * scale, height: Double(height) * scale)
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(px.width), pixelsHigh: Int(px.height), bitsPerSample: 8,
                                         samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
        else { return Data() }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        (dark ? NSColor(white: 0.18, alpha: 1) : NSColor(white: 0.94, alpha: 1)).setFill()
        NSBezierPath(roundedRect: NSRect(origin: .zero, size: px), xRadius: 12 * scale, yRadius: 12 * scale).fill()
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 22 * scale, weight: .semibold),
                                                    .foregroundColor: dark ? NSColor.white : NSColor.black]
        NSAttributedString(string: "stub picture · Spend this week", attributes: attrs).draw(at: NSPoint(x: 24 * scale, y: px.height / 2 - 14 * scale))
        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:]) ?? Data()
    }

    static func runPictures() -> Never {
        FenceSnapshots.shared.draw = { try await stubDraw($0) }
        func frame(_ card: AnswerCard, _ tag: String) {
            let states = FenceSnapshots.shared.cache.states.map { k, v -> String in
                switch v {
                case .drawing: return "\(k.lang): drawing"
                case .image(let png, let w, let h): return "\(k.lang): image \(Int(w))×\(Int(h)) (\(png.count) bytes)"
                case .fallback(let why): return "\(k.lang): fallback (\(why))"
                }
            }.sorted()
            log("\(tag): fences [\(states.joined(separator: "; "))]")
            MarkdownStreamProbe.snapshot(card, label: tag, png: URL(fileURLWithPath: "/tmp/192-pictures-\(tag).png"), prefix: "probe-pictures")
        }
        log("segments while streaming: \(AnswerSegments.split(head, finished: false).map(describe).joined(separator: " | "))")

        let card = MarkdownStreamProbe.card()
        var s = AnswerCardState()
        s.begin(question: "show me the week", kind: .ask)
        s.text = head; s.phase = .answer
        card.state = s
        card.show(near: .zero, key: false)
        let t0 = Date()
        for t in [0.2, 0.6, 1.2] { MarkdownStreamProbe.spin(until: t0.addingTimeInterval(t)); frame(card, "stream+\(t)s") }

        s = card.state // the third fence closes and the reply finishes
        s.text = head + "\n```\nThat's all."; s.turnFinished = true
        card.state = s
        let t1 = Date()
        for t in [0.6, 1.5] { MarkdownStreamProbe.spin(until: t1.addingTimeInterval(t)); frame(card, "finished+\(t)s") }
        card.hide(returnKey: false)

        log("a second card with the same reply: every fence a cache hit, no request")
        let again = MarkdownStreamProbe.card()
        again.state = s
        again.show(near: .zero, key: false)
        MarkdownStreamProbe.spin(0.6)
        frame(again, "reopened+0.6s")
        again.hide(returnKey: false)
        log("done")
        exit(0)
    }

    static func runThinking() -> Never {
        for kind in [AnswerCardState.Kind.ask, .voiceNote] { thinking(kind) }
        log("done", prefix: "probe-thinking")
        exit(0)
    }

    /// Reasoning streams for 2.4 s (16 fragments, more than the six-line tail), then the answer's first text, then the finish.
    private static func thinking(_ kind: AnswerCardState.Kind) {
        let name = kind == .ask ? "ask" : "voice"
        let card = MarkdownStreamProbe.card()
        var router = AskRouter()
        router.sent(on: "desk", question: "what changed?")
        var s = AnswerCardState()
        s.begin(question: "what changed?", kind: kind)
        s.status = "thinking"
        card.state = s
        card.show(near: .zero, key: false)
        func apply() {
            guard let r = router.reply else { return }
            var s = card.state
            s.text = r.text; s.thinking = r.thinking; s.turnFinished = r.finished
            s.phase = r.finished || !r.text.isEmpty ? .answer : .thinking
            card.state = s
        }
        func frame(_ tag: String) {
            let t = card.state.thinking
            log("\(name) \(tag): thinking \(t.live ? "live" : (t.summary ?? "none")) · \(t.text.count) chars", prefix: "probe-thinking")
            MarkdownStreamProbe.snapshot(card, label: "\(name) \(tag)", png: URL(fileURLWithPath: "/tmp/192-thinking-\(name)-\(tag).png"), prefix: "probe-thinking")
        }
        MarkdownStreamProbe.spin(0.3)
        frame("spinner")
        let t0 = Date()
        var shot = [0.6, 1.5, 2.2]
        for i in 0..<16 {
            MarkdownStreamProbe.spin(until: t0.addingTimeInterval(Double(i) * 0.15))
            router.observe(.reasoningDelta(session: "desk", text: "Step \(i + 1): reading the board and the last three receipts. "))
            apply()
            if let next = shot.first, Date().timeIntervalSince(t0) >= next { shot.removeFirst(); frame("thinking+\(next)s") }
        }
        MarkdownStreamProbe.spin(until: t0.addingTimeInterval(2.6))
        router.observe(.textDelta(session: "desk", text: "Two things changed since this morning:\n\n- w3 merged\n- the build is green"))
        apply()
        MarkdownStreamProbe.spin(0.4)
        frame("answer+0.4s")
        router.observe(.turnFinished(session: "desk", stopReason: "end_turn"))
        router.caughtUp(id: router.reply!.id, loaded: nil, trigger: .turnFinished, now: Date()) // the probe's "read failed" finish
        apply()
        MarkdownStreamProbe.spin(0.6)
        frame("finished+0.6s")
        card.hide(returnKey: false)
        MarkdownStreamProbe.spin(0.3)
    }

    private static func describe(_ s: AnswerSegment) -> String {
        switch s {
        case .markdown(let t): return "markdown \(t.count) chars"
        case .picture(let f): return "picture \(f.lang)"
        case .drawing(let lang, let title): return "drawing \(lang)\(title.map { " “\($0)”" } ?? "")"
        }
    }

    private static func log(_ s: String, prefix: String = "probe-pictures") { Log.write("\(prefix): \(s)") }
}
