import AppKit
import Combine
import CompanionCore
import KeyboardShortcuts

extension AppDelegate {
    /// W3: the Ask bar panel, the key-down half of the hotkey (key-up stays in W1's handler), `--ask`, `--ask-probe N`.
    func installAskBar(attention: PanelController) {
        askBar = AskBarController(store: store, attention: attention)
        KeyboardShortcuts.onKeyDown(for: .openPanel) { [weak self] in
            guard let self, Flags.probe == nil else { return }
            self.askBar.hotkeyDown()
        }
        if let seconds = AskFlags.probe { AskProbe.run(seconds: seconds, store: store, askBar: askBar) }
        else if AskFlags.ask { askBar.open() }
    }
}

/// `--ask-probe N --shareable`: the Ask bar on screen (not key, mouse-transparent) with "probe — do not act",
/// theuth's desk resolved through `mesh_status` + `desk_open("theuth")` (opens the desk, sends nothing), the
/// desk id and hub-ness logged to `/tmp/130-w3-probe.log`, quit after N s. Never sends, never records.
@MainActor
enum AskProbe {
    private static var subs = Set<AnyCancellable>()

    static func run(seconds: TimeInterval, store: Store, askBar: AskBarController) {
        AskLog.write("ask-probe start seconds=\(seconds) shareable=\(Flags.shareable) bundle=\(Bundle.main.bundleIdentifier ?? "nil") mic-usage-key=\(Bundle.main.object(forInfoDictionaryKey: "NSMicrophoneUsageDescription") != nil)")
        var resolved = false
        store.$connection.receive(on: RunLoop.main).sink { s in
            MainActor.assumeIsolated {
                guard s == .open, !resolved else { return }
                resolved = true
                Task {
                    do {
                        let desk = try await store.resolveDesk()
                        let hub: String
                        if case .ready(_, let h) = store.ask.router.desk { hub = "\(h)" } else { hub = "?" }
                        AskLog.write("ask-probe desk=\(desk) this-engine-is-hub=\(hub) (nothing sent)")
                    } catch {
                        AskLog.write("ask-probe desk unresolved: \(error)")
                    }
                }
                if AskFlags.syntheticDictation { runDictation(store: store, askBar: askBar) }
                if AskFlags.probeSpeak { Task { await speakScenario(store: store, askBar: askBar) } }
            }
        }.store(in: &subs)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
            askBar.showWithoutKey(text: "probe — do not act")
            AskLog.write("ask-probe panel frame=\(askBar.panel.frame) visible=\(askBar.panel.isVisible) key=\(askBar.panel.isKeyWindow) ignoresMouse=\(askBar.panel.ignoresMouseEvents) active=\(NSApp.isActive)")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
            let wins = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
                .filter { ($0[kCGWindowOwnerPID as String] as? Int32) == getpid() }
                .compactMap { $0[kCGWindowBounds as String] as? [String: Any] }
                .map { "x=\($0["X"] ?? 0) y=\($0["Y"] ?? 0) w=\($0["Width"] ?? 0) h=\($0["Height"] ?? 0)" }
            AskLog.write("ask-probe windows (CGWindowList): \(wins)")
            AskLog.write("ask-probe done after \(seconds) s — quitting without sending (receipts=\(store.ask.receipts.count) frontmost=\(NSWorkspace.shared.frontmostApplication?.localizedName ?? "?") active=\(NSApp.isActive) key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none") askbar key=\(askBar.panel.isKeyWindow))")
            NSApp.terminate(nil)
        }
    }

    // MARK: --synthetic-dictation (W6): 2 s of a 440 Hz tone through dictation_start/audio/stop, every event logged

    static let dictationLog = URL(fileURLWithPath: "/tmp/130-w6-dictation.log")

    private static func dlog(_ s: String) {
        AskLog.write(s)
        let line = "[companion \(Date()) pid \(getpid())] \(s)\n"
        if let h = try? FileHandle(forWritingTo: dictationLog) { h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close() }
        else { try? Data(line.utf8).write(to: dictationLog) }
    }

    private static func runDictation(store: Store, askBar: AskBarController) {
        try? FileManager.default.removeItem(at: dictationLog)
        dlog("synthetic-dictation start: 2 s of 440 Hz at 16 kHz, 100 ms chunks, microphone untouched, nothing sent")
        store.onDictationWire = { line in dlog("event \(line)") }
        askBar.probeFailed = { why in dlog("dictation_start refused: \(why) → state error=\(store.ask.dictation.error ?? "nil") phase=\(store.ask.dictation.phase)") }
        askBar.probeDictation(seconds: 2) {
            dlog("stop sent; waiting for DictationFinished")
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
                let d = store.ask.dictation
                dlog("after stop: phase=\(d.phase) utterances=\(d.utterances) liveText=\"\(d.liveText)\" error=\(d.error ?? "nil")")
                Task { await holdScenarios(store: store, askBar: askBar) }
            }
        }
    }

    /// 149g F2: the Ask bar's own hold machine (`voiceEvent`, what the hotkey calls) through its exit paths while
    /// `dictation_start` is still in flight (`probeStartDelay` 1.5 s), each followed by `dictation_status`:
    /// 1. CANCEL IN FLIGHT — ⎋/close (`close` → `.failed("cancelled")`) 400 ms into the hold: the late start stops its own id.
    /// 2. RELEASE BEFORE START — key-up into the send path before the id exists: the start adopts, stops, nothing recognized.
    /// 3. REFUSED — the probe holds the slot; the hold's start is refused: the hold ends with its hint, the probe's dictation untouched.
    /// No microphone (`AskFlags.automated`), no send (`.setText` is suppressed). Whatever leaks is stopped.
    private static func holdScenarios(store: Store, askBar: AskBarController) async {
        func active() async -> String? {
            do { return try await store.client.dictationActive() } catch { dlog("dictation_status failed: \(error)"); return "rpc-failed" }
        }
        func settle(_ s: Double = 1) async { try? await Task.sleep(for: .seconds(s)) }
        func cleanup(_ a: String?) async {
            guard let leaked = a, leaked != "rpc-failed" else { return }
            do { try await store.client.dictationStop(id: leaked); dlog("cleanup: stopped the leaked dictation \(leaked)") }
            catch { dlog("cleanup: dictation_stop \(leaked) failed: \(error)") }
        }
        /// Key-down; the 50 ms tick timer passes `holdThreshold` and `.startRecording` begins the (delayed) start.
        func pressUntilRecording() async {
            askBar.voiceEvent(.keyDown(Date()))
            await settle(0.4)
            dlog("hold: 400 ms in → voice=\(askBar.voiceState) phase=\(store.ask.dictation.phase)")
        }

        dlog("CANCEL IN FLIGHT: dictation_start held 1.5 s; key-down, close (⎋'s path) at 400 ms")
        askBar.probeStartDelay = .milliseconds(1500)
        await pressUntilRecording()
        askBar.close(returnKey: false)
        askBar.showWithoutKey(text: "probe — do not act")
        await settle(2.5)
        let a1 = await active()
        dlog("CANCEL IN FLIGHT result: voice=\(askBar.voiceState) phase=\(store.ask.dictation.phase) status=\"\(askBar.probeStatus ?? "")\" dictation_status.active=\(a1 ?? "nil")")
        dlog("ASSERT CANCEL IN FLIGHT idle, slot free: \((askBar.voiceState == .idle && !store.ask.dictation.isActive && a1 == nil) ? "PASS" : "FAIL")")
        await cleanup(a1)

        dlog("RELEASE BEFORE START: dictation_start held 1.5 s; key-down, key-up into the send path at 400 ms")
        await pressUntilRecording()
        askBar.voiceEvent(.keyUp(Date()))
        dlog("RELEASE BEFORE START: released → voice=\(askBar.voiceState) phase=\(store.ask.dictation.phase)")
        askBar.probeStartDelay = .zero
        await settle(4)
        let a2 = await active()
        dlog("RELEASE BEFORE START result: voice=\(askBar.voiceState) phase=\(store.ask.dictation.phase) status=\"\(askBar.probeStatus ?? "")\" dictation_status.active=\(a2 ?? "nil")")
        dlog("ASSERT RELEASE BEFORE START idle, slot free: \((askBar.voiceState == .idle && !store.ask.dictation.isActive && a2 == nil) ? "PASS" : "FAIL")")
        await cleanup(a2)

        let held: String
        do { held = try await store.client.dictationStart() } catch { return dlog("REFUSED skipped: the probe could not take the slot: \(error)") }
        dlog("REFUSED: the probe holds dictation \(held); key-down, 1 s hold, key-up")
        await pressUntilRecording()
        await settle(0.6)
        askBar.voiceEvent(.keyUp(Date()))
        await settle()
        let a3 = await active()
        dlog("REFUSED result: voice=\(askBar.voiceState) phase=\(store.ask.dictation.phase) status=\"\(askBar.probeStatus ?? "")\" dictation_status.active=\(a3 ?? "nil") (the probe's \(held))")
        dlog("ASSERT REFUSED idle with the hint, the probe's dictation untouched: \((askBar.voiceState == .idle && askBar.probeStatus == "dictating elsewhere" && a3 == held) ? "PASS" : "FAIL")")
        do { try await store.client.dictationStop(id: held) } catch { dlog("dictation_stop \(held) failed: \(error)") }
        await settle()
        dlog("REFUSED: probe's dictation stopped → active=\(await active() ?? "nil")")
    }

    // MARK: --probe-speak (149s S3): the Ask reply spoken through the shared speaker, against the live engine

    static let speakLog = URL(fileURLWithPath: "/tmp/149s-speak-probe.log")

    private static func slog(_ s: String) {
        AskLog.write(s)
        let line = "[companion \(Date()) pid \(getpid())] \(s)\n"
        if let h = try? FileHandle(forWritingTo: speakLog) { h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close() }
        else { try? Data(line.utf8).write(to: speakLog) }
    }

    /// `askSpeak` is on through `AskFlags.probeSpeak` (the owner's defaults untouched). A fixture reply — `TextDelta` partials
    /// then `TurnFinished` on the resolved desk — runs through `askObserve`, the path a real reply takes; nothing is sent to the
    /// desk (`send_input` never called), but `speak` goes to the live engine and its `SpeechAudio` packets decode here into a
    /// MUTED player (volume 0: the probe never sounds on the owner's Mac). Asserts: `speak` called once with the final text, no
    /// speak on partials, packets decoded, then a second ask (`submitAsk` → `.escape` undoes the hold before it can send) stops
    /// the utterance and `stop_speaking` goes out for its id.
    private static func speakScenario(store: Store, askBar: AskBarController) async {
        try? FileManager.default.removeItem(at: speakLog)
        func settle(_ s: Double) async { try? await Task.sleep(for: .seconds(s)) }
        func event(_ type: String, _ session: String, _ extra: [String: String]) -> Data {
            var o = extra; o["type"] = type; o["session"] = session
            return try! JSONSerialization.data(withJSONObject: o)
        }
        guard let desk = store.ask.router.deskSession else { return slog("SPEAK skipped: no desk") }
        store.speech.volume = 0
        var speaks: [String] = [], stops: [String] = []
        store.speaker.log = { line in
            if line.hasPrefix("speak(") { speaks.append(line) }
            if line.hasPrefix("stop ") { stops.append(line) }
            slog("speaker: \(line)")
        }
        slog("SPEAK: askSpeak=\(store.askSpeakEnabled) (override, defaults untouched: askSpeak key=\(Settings.defaults.object(forKey: SpokenReplies.askKey).map { "\($0)" } ?? "unset")) desk=\(desk.prefix(8))… muted=true")
        let text = "Probe reply: the spoken-reply path works end to end."
        store.ask.router.sent(on: desk) // what `commitAsk` does after `send_input`; the send itself never happens here
        store.askObserve(event("TextDelta", desk, ["text": "Probe reply: "]))
        store.askObserve(event("TextDelta", desk, ["text": "the spoken-reply path "]))
        slog("partials fed → speaks=\(speaks.count) surface=\(String(describing: store.speaker.surface))")
        let noPartialSpeak = speaks.isEmpty
        store.askObserve(event("TextDelta", desk, ["text": "works end to end."]))
        store.askObserve(event("TurnFinished", desk, ["stop_reason": "end_turn"]))
        await settle(4)
        let decoded = store.speech.decoded
        slog("final fed → speaks=\(speaks) id=\(store.speaker.id ?? "nil") surface=\(String(describing: store.speaker.surface)) playing=\(store.speech.isPlaying) decoded=\(decoded) receipt=\(String(describing: store.receipt))")
        slog("ASSERT speak once with the final text, none for partials: \((noPartialSpeak && speaks.count == 1 && speaks[0].hasPrefix("speak(askBar) \(text.count) chars")) ? "PASS" : "FAIL")")
        slog("ASSERT SpeechAudio received and decoded: \(decoded > 0 ? "PASS" : "FAIL") (\(decoded) packets)")
        let id = store.speaker.id
        store.submitAsk("probe — second ask, undone")
        store.perform(.escape) // undo the hold: nothing reaches the desk
        await settle(1)
        slog("second ask → surface=\(String(describing: store.speaker.surface)) playing=\(store.speech.isPlaying) stops=\(stops) receipt=\(String(describing: store.receipt))")
        slog("ASSERT a second ask stops it (stop_speaking for its id): \((store.speaker.surface == nil && !store.speech.isPlaying && id != nil && stops == ["stop \(id!)"]) ? "PASS" : "FAIL")")
        slog("SPEAK done — nothing sent (receipts=\(store.ask.receipts.count))")
    }
}
