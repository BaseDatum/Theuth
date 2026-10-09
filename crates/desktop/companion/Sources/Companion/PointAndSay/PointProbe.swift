import AppKit
import Combine
import CompanionCore
import TheuthKit

/// `--probe-point N --shareable` (§10): the overlays revealed with a synthetic 2 s spiral on every display and the voice
/// pill beside the pointer; a CONTROL `screen_context` with the overlays up (the detector must see malachite along the
/// spiral), then the Stop sequence — a synthetic 16 kHz tone through `dictation_start/audio/stop`, every overlay ordered
/// out and one run-loop turn awaited, a REAL `screen_context` — and the assertion that no malachite remains along the
/// spiral; the answer card shown (not key) with a canned text for the screenshot; quit after N s. STOPS before
/// `desk_open`: nothing is sent, the microphone is never opened, nothing takes key. Log `/tmp/149-W2-probe.log`.
///
/// The engine stamps the request's path on its capture in the request's colour (`context.rs:25-29`), so the request
/// carries a short segment away from the spiral in carnelian: every malachite pixel along the spiral is ours.
@MainActor
enum PointProbe {
    private static var subs = Set<AnyCancellable>()
    static let overviewCopy = URL(fileURLWithPath: "/tmp/149-W2-probe-overview.png")

    static func run(seconds: TimeInterval, store: Store, controller c: PointController) {
        try? FileManager.default.removeItem(at: PointLog.file)
        PointLog.write("probe-point start seconds=\(seconds) shareable=\(Flags.shareable) bundle=\(Bundle.main.bundleIdentifier ?? "nil") screens=\(NSScreen.screens.count) mic-usage-key=\(Bundle.main.object(forInfoDictionaryKey: "NSMicrophoneUsageDescription") != nil)")
        var started = false
        store.$connection.receive(on: RunLoop.main).sink { s in
            MainActor.assumeIsolated {
                guard s == .open, !started else { return }
                started = true
                Task { await sequence(store: store, controller: c) }
            }
        }.store(in: &subs)
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
            PointLog.write("probe-point done after \(seconds) s — quitting without sending (overlays visible=\(c.overlays.revealed) card visible=\(c.card.isVisible) key=\(c.card.panel.isKeyWindow) active=\(NSApp.isActive) frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?"))")
            NSApp.terminate(nil)
        }
    }

    // MARK: the spiral

    /// Three turns, 20 → 160 pt, at the display's centre; `n` points in that display's global logical frame.
    static func spiral(in d: OverlayDisplay, n: Int) -> [CGPoint] {
        (0..<n).map { i in
            let f = Double(i) / Double(max(1, n - 1))
            let r = 20 + 140 * f, a = f * 3 * 2 * .pi
            return CGPoint(x: d.frame.midX + r * cos(a), y: d.frame.midY + r * sin(a))
        }
    }

    private static func sequence(store: Store, controller c: PointController) async {
        let displays = PointerSampler.displays()
        let perDisplay = max(30, 120 / max(1, displays.count))
        let spirals = displays.map { spiral(in: $0, n: perDisplay) }
        var samples: [PointerSample] = []
        var t: UInt32 = 0
        for pts in spirals {
            for p in pts { samples.append(PointerSample(tMs: t, x: Float(p.x), y: Float(p.y), pen: true)); t += 1000 / 60 }
        }
        c.overlays.reveal(catchMouse: false)
        c.pill.state.reset(chord: "⌃S")
        c.pill.state.text = "probe — do not act"
        c.pill.show(near: NSEvent.mouseLocation)
        PointLog.write("overlays revealed: \(c.overlays.count) displays \(c.overlays.frames.map { "\($0.key)=\($0.value)" }.sorted()) pill=\(c.pill.panel.frame)")
        // Animate the trail at 60 Hz for ~2 s, display after display.
        for i in stride(from: 1, through: samples.count, by: 1) {
            c.overlays.draw(samples: Array(samples.prefix(i)), displays: displays)
            try? await Task.sleep(for: .milliseconds(1000 / 60))
        }
        PointLog.write("spiral drawn: \(samples.count) samples on \(displays.count) displays")
        // The request path: one short pen-down segment per display, far from the spiral, so the engine's own stamp (in
        // carnelian) never lands where the detector looks.
        let reqSamples: [PointerSample] = displays.enumerated().flatMap { i, d in
            (0..<12).map { k in
                PointerSample(tMs: UInt32(i * 200 + k * 16), x: Float(d.frame.minX + d.frame.width * 0.15 + Double(k) * 4), y: Float(d.frame.minY + d.frame.height * 0.85), pen: true)
            }
        }
        let carnelian = "#c45a48"

        // Control: overlays up → the detector must see the trail.
        let control = await c.screenContext(samples: reqSamples, durationMs: 2000, color: carnelian)
        let before = count(reply: control, spirals: spirals, displays: displays)
        PointLog.write("control (overlays visible): malachite \(before.map { "\($0.hit)/\($0.total)" }) files=\(control?.files ?? [])")

        // The Stop sequence: tone in, overlays out + one run-loop turn, dictation_stop ∥ screen_context.
        c.beginDictation()
        for i in 0..<20 {
            let start = i * VoiceRecorder.chunk
            c.feed((0..<VoiceRecorder.chunk).map { j in Int16(sin(2 * .pi * 440 * Double(start + j) / VoiceRecorder.sampleRate) * 0.3 * 32767) })
            try? await Task.sleep(for: .milliseconds(100))
        }
        PointLog.write("tone fed: 2 s of 440 Hz at 16 kHz in 100 ms chunks, dictation id=\(c.dictation.id ?? "nil") phase=\(c.dictation.phase)")
        c.overlays.conceal()
        await PointController.runLoopTurn()
        let mine = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
            .filter { ($0[kCGWindowOwnerPID as String] as? Int32) == getpid() }.count
        PointLog.write("overlays ordered out before the capture: revealed=\(c.overlays.revealed) own on-screen windows=\(mine)")
        c.endDictation()
        async let text = c.finalText()
        async let ctx = c.screenContext(samples: reqSamples, durationMs: 2000, color: carnelian)
        let (t2, reply) = await (text, ctx)
        let after = count(reply: reply, spirals: spirals, displays: displays)
        let pass = !before.isEmpty && before.contains { $0.hit * 5 >= $0.total } && after.allSatisfy { $0.hit * 50 <= $0.total }
        PointLog.write("stop (overlays out): malachite \(after.map { "\($0.hit)/\($0.total)" }) lines=\(reply?.lines ?? []) files=\(reply?.files ?? [])")
        PointLog.write("ASSERT no malachite along the path after the order-out: \(pass ? "PASS" : "FAIL") (control ≥ 20 % hit on some display, stop ≤ 2 % on every display)")
        PointLog.write("dictation after stop: text=\"\(t2)\" phase=\(c.dictation.phase) error=\(c.dictation.error ?? "nil")")
        if let f = reply?.files.first { try? FileManager.default.removeItem(at: overviewCopy); try? FileManager.default.copyItem(atPath: f, toPath: overviewCopy.path); PointLog.write("overview copied to \(overviewCopy.path)") }
        // The answer card, not key, with a canned reply (nothing was sent).
        c.pill.hide()
        c.card.state.begin(question: "probe — do not act")
        c.card.state.status = "replied"
        c.card.state.text = "Probe: this card shows the desk's streamed reply. Nothing was sent — the probe stopped before desk_open."
        c.card.show(near: NSEvent.mouseLocation, key: false)
        PointLog.write("answer card shown (not key): frame=\(c.card.panel.frame) visible=\(c.card.isVisible) key=\(c.card.panel.isKeyWindow); stopped before desk_open — nothing sent")

        // Gesture phase: the real driver — press → Armed → tick → Holding → the sampler over the real pointer, the pill
        // following it, the tone through `feed` → release → the Stop sequence — with `dryRun`, which stops it before
        // `desk_open` even if the model hears words in a tone. No hotkey, no microphone, no Esc registration.
        try? await Task.sleep(for: .seconds(4))
        c.card.hide(returnKey: false)
        c.dryRun = true
        c.send(.pressed)
        try? await Task.sleep(for: .milliseconds(400))
        PointLog.write("gesture: after 400 ms state=\(c.state) overlays=\(c.overlays.revealed) pill=\(c.pill.isVisible) samples=\(c.sampler.samples.count) point.slice=\(store.point.map { "\($0.state)" } ?? "nil")")
        for i in 0..<20 {
            let start = i * VoiceRecorder.chunk
            c.feed((0..<VoiceRecorder.chunk).map { j in Int16(sin(2 * .pi * 440 * Double(start + j) / VoiceRecorder.sampleRate) * 0.3 * 32767) })
            try? await Task.sleep(for: .milliseconds(100))
        }
        let sampled = c.sampler.samples.count
        c.send(.released)
        try? await Task.sleep(for: .seconds(3))
        PointLog.write("gesture: released after ~2.4 s with \(sampled) samples → state=\(c.state) overlays=\(c.overlays.revealed) card=\(c.card.isVisible) pill=\(c.pill.isVisible) hint=\"\(c.pill.state.hint)\" point.slice=\(store.point.map { "\($0.state)" } ?? "nil")")
    }

    // MARK: the detector

    struct Count { var hit = 0; var total = 0 }

    /// Per visited display (the overviews come first, in visit order): the spiral's points mapped into the PNG, classified
    /// as malachite-ish (green well above red and blue; the trail is 0.85 malachite over whatever is under it).
    static func count(reply: ScreenContextReply?, spirals: [[CGPoint]], displays: [OverlayDisplay]) -> [Count] {
        guard let reply else { return [] }
        return displays.indices.compactMap { i -> Count? in
            guard i < reply.files.count, let img = NSBitmapImageRep(data: (try? Data(contentsOf: URL(fileURLWithPath: reply.files[i]))) ?? Data()) else { return nil }
            let d = displays[i]
            let sx = Double(img.pixelsWide) / d.frame.width, sy = Double(img.pixelsHigh) / d.frame.height
            var c = Count()
            for p in spirals[i] {
                let x = Int((p.x - d.frame.minX) * sx), y = Int((p.y - d.frame.minY) * sy)
                guard x >= 0, y >= 0, x < img.pixelsWide, y < img.pixelsHigh, let col = img.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                c.total += 1
                let r = col.redComponent * 255, g = col.greenComponent * 255, b = col.blueComponent * 255
                if g >= 55, g > r + 25, g > b + 5 { c.hit += 1 }
            }
            return c
        }
    }
}
