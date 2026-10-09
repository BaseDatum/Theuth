import AppKit
import Combine
import CompanionCore
import TheuthKit

/// `--probe-point N --shareable` (§10): the overlays revealed with a synthetic 2 s spiral on every display and the voice
/// pill beside the pointer; a CONTROL `screen_context` with the overlays up (the detector must see the accent colour along
/// the spiral), then the Stop sequence — a synthetic 16 kHz tone through `dictation_start/audio/stop`, every overlay
/// ordered out and one run-loop turn awaited, a REAL `screen_context` — and the assertion that no trail remains along the
/// spiral; the answer card shown (not key) with a canned text for the screenshot; quit after N s. STOPS before
/// `desk_open`: nothing is sent, the microphone is never opened, nothing takes key. Log `/tmp/149-W2-probe.log`.
///
/// The engine stamps the request's path on its capture in the request's colour (`context.rs:25-29`), so the request
/// carries a short segment away from the spiral in a fixed CONTROL colour (the accent's complement, never shipped on
/// screen outside this probe): every accent-coloured pixel along the spiral is ours. (149n: was malachite vs carnelian.)
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
        // the control colour) never lands where the detector looks. (Explicit loop: the flatMap/map closure form was
        // too complex for the type checker to build — Sky's fix da6afe49.)
        var reqSamples: [PointerSample] = []
        for (i, d) in displays.enumerated() {
            let f: CGRect = d.frame
            let x0: Double = f.minX + f.width * 0.15
            let y: Float = Float(f.minY + f.height * 0.85)
            for k in 0..<12 {
                let t = UInt32(i * 200 + k * 16)
                let x = Float(x0 + Double(k) * 4)
                reqSamples.append(PointerSample(tMs: t, x: x, y: y, pen: true))
            }
        }
        let accent = accentRGB
        let control = OverlayModel.hex(red: 1 - accent.r, green: 1 - accent.g, blue: 1 - accent.b) // the control colour: the accent's complement

        // Control: overlays up → the detector must see the trail.
        let controlReply = await c.screenContext(samples: reqSamples, durationMs: 2000, color: control)
        let before = count(reply: controlReply, spirals: spirals, displays: displays)
        PointLog.write("control (overlays visible): accent \(before.map { "\($0.hit)/\($0.total)" }) files=\(controlReply?.files ?? [])")

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
        async let ctx = c.screenContext(samples: reqSamples, durationMs: 2000, color: control)
        let (t2, reply) = await (text, ctx)
        let after = count(reply: reply, spirals: spirals, displays: displays)
        let pass = !before.isEmpty && before.contains { $0.hit * 5 >= $0.total } && after.allSatisfy { $0.hit * 50 <= $0.total }
        PointLog.write("stop (overlays out): accent \(after.map { "\($0.hit)/\($0.total)" }) lines=\(reply?.lines ?? []) files=\(reply?.files ?? [])")
        PointLog.write("ASSERT no accent trail along the path after the order-out: \(pass ? "PASS" : "FAIL") (control ≥ 20 % hit on some display, stop ≤ 2 % on every display)")
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
        c.keyDown()
        try? await Task.sleep(for: .milliseconds(400))
        PointLog.write("gesture: after 400 ms state=\(c.state) overlays=\(c.overlays.revealed) pill=\(c.pill.isVisible) samples=\(c.sampler.samples.count) point.slice=\(store.point.map { "\($0.state)" } ?? "nil")")
        for i in 0..<20 {
            let start = i * VoiceRecorder.chunk
            c.feed((0..<VoiceRecorder.chunk).map { j in Int16(sin(2 * .pi * 440 * Double(start + j) / VoiceRecorder.sampleRate) * 0.3 * 32767) })
            try? await Task.sleep(for: .milliseconds(100))
        }
        let sampled = c.sampler.samples.count
        c.keyUp()
        try? await Task.sleep(for: .seconds(3))
        PointLog.write("gesture: released after ~2.4 s with \(sampled) samples → state=\(c.state) overlays=\(c.overlays.revealed) card=\(c.card.isVisible) pill=\(c.pill.isVisible) hint=\"\(c.pill.state.hint)\" point.slice=\(store.point.map { "\($0.state)" } ?? "nil")")
        await holdScenarios(store: store, controller: c)
    }

    static let holdCopy = URL(fileURLWithPath: "/tmp/149f-hold-probe.png")

    /// 100 ms of a 440 Hz tone, chunk `i`.
    static func tone(_ i: Int) -> [Int16] {
        let start = i * VoiceRecorder.chunk
        return (0..<VoiceRecorder.chunk).map { j in Int16(sin(2 * .pi * 440 * Double(start + j) / VoiceRecorder.sampleRate) * 0.3 * 32767) }
    }

    /// 149f (the 2026-10-09 voice-key report), through the hot key's own entry points with what Carbon delivers while the chord
    /// is held — a key-down, then a key-down every 30 ms (auto-repeat), then the key-up:
    /// 1. HOLD 1.5 s with the tone: ONE gesture, the Stop sequence, `dictation_status.active` nil afterwards.
    /// 2. REFUSED HOLD: the probe takes the engine's one dictation slot first, so the gesture's `dictation_start` is refused
    ///    ("already dictating" → "dictating elsewhere"): the gesture must end ONCE and stay ended for the rest of the hold —
    ///    not start/refuse/cancel at the repeat rate — and the probe's own dictation must be untouched.
    /// 3. CANCEL IN FLIGHT: key-down, Esc before `dictation_start` replies, key-up: the dictation that lands must be stopped
    ///    (`active` nil within 1 s), not left on the engine until `dictation_max_secs`.
    /// 4. RELEASE BEFORE START: `dictation_start` held 1.5 s, a 1 s hold released into the Stop sequence: the late start must
    ///    adopt and stop itself; `active` nil afterwards.
    /// Whatever leaks is stopped at the end: this runs against the owner's live engine.
    private static func holdScenarios(store: Store, controller c: PointController) async {
        func active() async -> String? {
            do { return try await store.client.dictationActive() } catch { PointLog.write("dictation_status failed: \(error)"); return "rpc-failed" }
        }
        func hold(ms: Int, tone withTone: Bool, snapshot: Bool = false) async -> Int {
            c.keyDown()
            let t0 = Date()
            var repeats = 0
            var shot: Task<ScreenContextReply?, Never>?
            while Date().timeIntervalSince(t0) * 1000 < Double(ms) {
                try? await Task.sleep(for: .milliseconds(30))
                c.keyDown(); repeats += 1
                if withTone, repeats % 3 == 0 { c.feed(tone(repeats / 3)) }
                if snapshot, shot == nil, repeats == 25 {
                    let samples = c.sampler.samples
                    shot = Task { await c.screenContext(samples: samples, durationMs: 750) }
                }
            }
            c.keyUp()
            if let f = await shot?.value?.files.first {
                try? FileManager.default.removeItem(at: holdCopy); try? FileManager.default.copyItem(atPath: f, toPath: holdCopy.path)
                PointLog.write("HOLD mid-hold capture copied to \(holdCopy.path)")
            }
            return repeats
        }
        func settle(_ s: Double = 1) async { try? await Task.sleep(for: .seconds(s)) }

        try? await Task.sleep(for: .seconds(1))
        let g0 = c.gesture
        PointLog.write("HOLD: key-down, auto-repeat key-down every 30 ms for 1.5 s with the tone, key-up")
        let r0 = await hold(ms: 1500, tone: true, snapshot: true)
        await settle(3)
        let a0 = await active()
        PointLog.write("HOLD result: \(r0) repeats → gestures=\(c.gesture - g0) state=\(c.state) hint=\"\(c.pill.state.hint)\" overlays=\(c.overlays.revealed) dictation.phase=\(c.dictation.phase) dictation_status.active=\(a0 ?? "nil")")
        PointLog.write("ASSERT HOLD one gesture, idle after the Stop sequence, slot free: \((c.gesture - g0 == 1 && c.state == .idle && a0 == nil) ? "PASS" : "FAIL")")

        let held: String
        do { held = try await store.client.dictationStart() } catch { PointLog.write("REFUSED HOLD skipped: the probe could not take the slot: \(error)"); return }
        PointLog.write("REFUSED HOLD: the probe holds dictation \(held); key-down, auto-repeat every 30 ms for 1.5 s, key-up")
        let g1 = c.gesture
        let r1 = await hold(ms: 1500, tone: false)
        await settle()
        let a1 = await active()
        PointLog.write("REFUSED HOLD result: \(r1) repeats → gestures=\(c.gesture - g1) state=\(c.state) hint=\"\(c.pill.state.hint)\" overlays=\(c.overlays.revealed) dictation_status.active=\(a1 ?? "nil") (the probe's \(held))")
        PointLog.write("ASSERT REFUSED HOLD one gesture, idle, the probe's dictation untouched: \((c.gesture - g1 == 1 && c.state == .idle && a1 == held) ? "PASS" : "FAIL")")
        do { try await store.client.dictationStop(id: held) } catch { PointLog.write("dictation_stop \(held) failed: \(error)") }
        await settle()
        PointLog.write("REFUSED HOLD: probe's dictation stopped → active=\(await active() ?? "nil")")

        PointLog.write("CANCEL IN FLIGHT: key-down, Esc before dictation_start replies, key-up")
        c.keyDown()
        c.send(.esc)
        c.keyUp()
        await settle()
        let a2 = await active()
        PointLog.write("CANCEL IN FLIGHT result: state=\(c.state) dictation.phase=\(c.dictation.phase) dictation_status.active=\(a2 ?? "nil")")
        PointLog.write("ASSERT CANCEL IN FLIGHT slot free within 1 s: \(a2 == nil ? "PASS" : "FAIL")")
        if let leaked = a2, leaked != "rpc-failed" {
            do { try await store.client.dictationStop(id: leaked); PointLog.write("cleanup: stopped the leaked dictation \(leaked)") }
            catch { PointLog.write("cleanup: dictation_stop \(leaked) failed: \(error)") }
        }

        // 4. RELEASE BEFORE START (the Windows port's race, 149f addendum): the start is held 1.5 s; a 1 s hold releases into
        // the Stop sequence before the id exists. The late start must adopt and stop itself (`stopWanted`), the slot must be free.
        PointLog.write("RELEASE BEFORE START: dictation_start held 1.5 s; key-down, 1 s hold with the tone, key-up into send")
        c.probeStartDelay = .milliseconds(1500)
        let g3 = c.gesture
        _ = await hold(ms: 1000, tone: true)
        c.probeStartDelay = .zero
        await settle(4)
        let a3 = await active()
        PointLog.write("RELEASE BEFORE START result: gestures=\(c.gesture - g3) state=\(c.state) hint=\"\(c.pill.state.hint)\" dictation.phase=\(c.dictation.phase) dictation_status.active=\(a3 ?? "nil")")
        PointLog.write("ASSERT RELEASE BEFORE START idle, slot free: \((c.gesture - g3 == 1 && c.state == .idle && a3 == nil) ? "PASS" : "FAIL")")
        if let leaked = a3, leaked != "rpc-failed" {
            do { try await store.client.dictationStop(id: leaked); PointLog.write("cleanup: stopped the leaked dictation \(leaked)") }
            catch { PointLog.write("cleanup: dictation_stop \(leaked) failed: \(error)") }
        }
    }

    // MARK: the detector

    struct Count { var hit = 0; var total = 0 }

    /// The trail's colour as drawn (`PointController.accentSRGB`), as components.
    static var accentRGB: (r: Double, g: Double, b: Double) {
        let c = PointController.accentSRGB
        return (c.redComponent, c.greenComponent, c.blueComponent)
    }

    /// Per visited display (the overviews come first, in visit order): the spiral's points mapped into the PNG, classified
    /// as accent-ish: within 0.4 (RGB Euclidean, 0...1) of the accent expressed in the capture's own colour space — the trail
    /// is 0.85 accent over whatever is under it, and the capture's P3 → sRGB round trip desaturates the blue a little more
    /// (measured 0.29 from the nominal accent over the dark desk). `colorAt` is compared raw: a second conversion only adds error.
    static func count(reply: ScreenContextReply?, spirals: [[CGPoint]], displays: [OverlayDisplay]) -> [Count] {
        guard let reply else { return [] }
        return displays.indices.compactMap { i -> Count? in
            guard i < reply.files.count, let img = NSBitmapImageRep(data: (try? Data(contentsOf: URL(fileURLWithPath: reply.files[i]))) ?? Data()) else { return nil }
            let d = displays[i]
            let sx = Double(img.pixelsWide) / d.frame.width, sy = Double(img.pixelsHigh) / d.frame.height
            let accent = PointController.accentSRGB.usingColorSpace(img.colorSpace) ?? PointController.accentSRGB
            var c = Count()
            for p in spirals[i] {
                let x = Int((p.x - d.frame.minX) * sx), y = Int((p.y - d.frame.minY) * sy)
                guard x >= 0, y >= 0, x < img.pixelsWide, y < img.pixelsHigh, let col = img.colorAt(x: x, y: y) else { continue }
                c.total += 1
                let dr = col.redComponent - accent.redComponent, dg = col.greenComponent - accent.greenComponent, db = col.blueComponent - accent.blueComponent
                if (dr * dr + dg * dg + db * db).squareRoot() <= 0.4 { c.hit += 1 }
            }
            return c
        }
    }
}
