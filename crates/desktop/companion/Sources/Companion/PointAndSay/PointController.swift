import AppKit
import Combine
import CompanionCore
import KeyboardShortcuts
import TheuthKit

extension KeyboardShortcuts.Name {
    /// The voice key (`KeyAction.voiceKey`, ⌃S on macOS): the chord comes from the table, never a library default.
    static let voiceKey = Self("voiceKey")
    /// ⎋ between press and send only (§8's pointAndSay row; `DESK/point/mod.rs:38-42`'s rule) — enabled/disabled per gesture.
    static let pointEscape = Self("pointEscape", default: .init(.escape))
}

@MainActor private var pointAndSay: PointController?

extension AppDelegate {
    /// 149 W2: the voice key, overlays, dictation, screen context, the answer card, the spoken reply; `--probe-point`.
    func installPointAndSay() {
        let c = PointController(store: store)
        pointAndSay = c
        if let seconds = Flags.probePoint { PointProbe.run(seconds: seconds, store: store, controller: c) }
    }
}

extension Store {
    /// Every raw event (after `askObserve`): `Dictation*` for the voice key's own session, the desk's reply, `Speech*`.
    func pointObserve(_ data: Data) { pointAndSay?.observe(data) }
}

extension Flags {
    /// `--probe-point N`: the spiral on every display, the Stop sequence with a synthetic tone and a real `screen_context`,
    /// quit after N s; nothing sent (§10).
    static let probePoint: TimeInterval? = {
        guard let i = args.firstIndex(of: "--probe-point"), i + 1 < args.count else { return nil }
        return TimeInterval(args[i + 1])
    }()
}

/// 149 §3.7 / §5.4 D: the driver around `PointMachine` — the hotkey's key-down/key-up and ⎋ in, the effects out. Owns the
/// timers (`after`), the sampler, the overlays, the voice pill, its own dictation session (`AskDictation`, the Ask bar's
/// reducer with the companion's PCM — D17/Revision 1 row 11), the Stop sequence and the answer card.
@MainActor
final class PointController {
    /// 96's `FINAL_TEXT_MS`: how long the final flush may take after `dictation_stop` before what we have is sent.
    static let finalTextMs: TimeInterval = 8

    private let store: Store
    private(set) var state = PointMachine.State.idle
    private var pressAt: UInt64 = 0
    /// Bumped on every `Start`: a Stop sequence that outlives its gesture drops its result.
    private var gesture = 0
    let sampler = PointerSampler()
    let overlays = OverlayWindows()
    let pill = VoicePill()
    let card = AnswerCard()
    let speech = SpeechPlayer()
    private let recorder = VoiceRecorder()
    private(set) var dictation = AskDictation()
    private var router = AskRouter()
    private var displays: [OverlayDisplay] = []
    private var subs = Set<AnyCancellable>()
    private var sendTask: Task<Void, Never>?
    private var pendingChunks: [[Int16]] = []
    private var stopWanted = false
    private var finalWaiter: CheckedContinuation<Void, Never>?
    private var doneTimer: Timer?
    private var speakingId: String?
    private var desk: String?
    /// The last `meeting_status` seen: a live meeting leases the mic, so a press is refused with the "Meeting running" hint.
    private var meetingLive = false
    private var prewarmed = false
    /// `--probe-point`'s gesture phase: the Stop sequence runs to the point of `desk_open` and stops there.
    var dryRun = false

    init(store: Store) {
        self.store = store
        // A probe registers nothing: a Carbon hot key swallows the chord for every app while the process lives.
        if !AskFlags.automated {
            applyHotkey(SettingsModel.shared.value)
            SettingsModel.shared.$value.removeDuplicates().dropFirst().receive(on: RunLoop.main)
                .sink { [weak self] v in MainActor.assumeIsolated { self?.applyHotkey(v) } }
                .store(in: &subs)
            KeyboardShortcuts.onKeyDown(for: .voiceKey) { [weak self] in self?.send(.pressed) }
            KeyboardShortcuts.onKeyUp(for: .voiceKey) { [weak self] in self?.send(.released) }
            KeyboardShortcuts.onKeyDown(for: .pointEscape) { [weak self] in self?.send(.esc) }
            KeyboardShortcuts.disable(.pointEscape)
        }
        recorder.onChunk = { [weak self] c in self?.feed(c) }
        card.onEscape = { [weak self] in self?.send(.esc) }
        card.onOpen = { [weak self] in
            guard let self, let d = self.desk else { return }
            self.store.onOpenURL?(DeepLink.session(d))
        }
        speech.onEnded = { [weak self] _ in
            guard let self else { return }
            self.speakingId = nil
            self.card.state.speaking = false
            if self.router.reply?.finished == true { self.armDone() }
        }
        // Installed before `store.start` (the insertion point); the client exists once the connection opens.
        store.$connection.removeDuplicates().receive(on: RunLoop.main)
            .sink { [weak self] s in MainActor.assumeIsolated { if s == .open { self?.refreshMeeting() } } }
            .store(in: &subs)
    }

    /// The table's `voiceKey` chord into KeyboardShortcuts; off = unregistered. NOT in `Hotkeys.names`: that launch probe
    /// would see Tauri's ⌃S (still bound until `companion-alive` lands, D22) and persist the row as unassigned.
    private func applyHotkey(_ v: CompanionSettings) {
        let want = VoiceKeySettings.enabled ? v.keys.chord(for: .voiceKey).map(Hotkeys.shortcut) : nil
        if KeyboardShortcuts.getShortcut(for: .voiceKey) != want { KeyboardShortcuts.setShortcut(want, for: .voiceKey) }
    }

    /// Settings › Voice key flipped "enabled".
    static func hotkeyChanged() { pointAndSay?.applyHotkey(SettingsModel.shared.value) }

    private var chordSymbol: String { SettingsModel.shared.value.keys.chord(for: .voiceKey)?.symbol ?? "⌃S" }

    private func refreshMeeting() {
        Task { [weak self] in
            guard let self, let live = try? await self.store.client.meetingStatus().state.isLive else { return }
            self.meetingLive = live
        }
    }

    // MARK: the machine

    func send(_ e: PointMachine.Event) {
        let now = PointClock.now()
        let refused: String? = (meetingLive || store.meeting?.isLive == true) ? "talking" : nil
        let (next, effects) = PointMachine.step(state, e, now: now, refused: refused)
        let prev = state
        state = next
        if prev != next { PointLog.write("point \(e) → \(next) \(effects)") }
        publish()
        for fx in effects { run(fx, now: now) }
        after(prev, next)
    }

    private func publish() {
        store.point = state == .idle ? nil : PointSlice(state: state, samples: sampler.samples, text: dictation.liveText)
    }

    private func run(_ fx: PointMachine.Effect, now: UInt64) {
        switch fx {
        case .start:
            gesture += 1
            pressAt = now
            displays = PointerSampler.displays()
            let mouse = VoiceKeySettings.annotateMouse
            overlays.reveal(catchMouse: mouse)
            sampler.onTick = { [weak self] s, _ in self?.tick(s) }
            sampler.start(t0: now, penAlways: !mouse, displays: displays)
            card.hide(returnKey: false) // a question from `.replying`: the old reply is over
            doneTimer?.invalidate(); doneTimer = nil
            stopSpeaking()
            router = AskRouter()
            pill.state.reset(chord: chordSymbol)
            pill.show(near: NSEvent.mouseLocation)
            KeyboardShortcuts.enable(.pointEscape)
            prewarm()
            beginDictation()
        case .showLock:
            pill.state.phase = .locked
        case .stop(let reason):
            let g = gesture
            Task { await self.stopSequence(gesture: g, reason: reason) }
        case .discard(let reason, let hint):
            teardownGesture()
            if let hint { pill.showHint(hint, near: NSEvent.mouseLocation) } else { pill.hide() }
            PointLog.write("point discarded: \(reason)")
        case .conceal:
            card.hide(returnKey: true)
            pill.hide()
            doneTimer?.invalidate(); doneTimer = nil
            stopSpeaking()
            router = AskRouter()
        case .refuse(let reason):
            pill.showHint(Self.hint(for: reason), near: NSEvent.mouseLocation)
            PointLog.write("point refused: \(reason)")
        }
    }

    /// Timers and the ⎋ lifetime follow the state, not the effects (`DESK/point/mod.rs:342-353`).
    private func after(_ prev: PointMachine.State, _ next: PointMachine.State) {
        switch next {
        case .armed(let t0): armTick(at: t0 + PointMachine.holdMs)
        case .tap1(_, let released): armTick(at: released + PointMachine.doubleTapMs + 1)
        case .idle, .replying:
            switch prev {
            case .idle, .replying: break
            default: KeyboardShortcuts.disable(.pointEscape); refreshMeeting()
            }
        default: break
        }
    }

    private func armTick(at: UInt64) {
        DispatchQueue.main.asyncAfter(deadline: DispatchTime(uptimeNanoseconds: at * 1_000_000)) { [weak self] in
            self?.send(.tick(now: at))
        }
    }

    /// 96's refusal copy (`voice-key.ts:329-343`).
    static func hint(for reason: String) -> String {
        if reason.contains("talking") { return "Meeting running — use the island to ask" }
        if reason.contains("already dictating") { return "dictating elsewhere" }
        return reason
    }

    private func tick(_ s: PointerSample?) {
        if s != nil { overlays.draw(samples: sampler.samples, displays: displays) }
        pill.follow(NSEvent.mouseLocation)
    }

    /// Everything a discarded gesture leaves behind: the sampler, the overlays, the mic, the dictation session.
    private func teardownGesture() {
        sampler.stop()
        overlays.conceal()
        recorder.cancel()
        if let id = dictation.id { enqueue { [store] in try await store.client.dictationStop(id: id) } }
        dictation = AskDictation()
        pendingChunks = []; stopWanted = false
        resumeFinal()
        publish()
    }

    // MARK: the Stop sequence (§5.4 D 4-6)

    private func stopSequence(gesture g: Int, reason: String) async {
        let samples = sampler.stop()
        let durationMs = PointClock.now() &- pressAt
        // D13: every overlay out and one run-loop turn awaited before the capture can start.
        overlays.conceal()
        await Self.runLoopTurn()
        recorder.stop() // the tail chunk goes out through `feed`
        endDictation()
        pill.state.phase = .sending
        async let text = finalText()
        async let context = screenContext(samples: samples, durationMs: durationMs)
        let (t, ctx) = await (text, context)
        guard g == gesture, case .sending = state else { return }
        let note = t.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !note.isEmpty else { return finish(hint: "Nothing recognized") }
        if dryRun { return finish(hint: "probe — stopped before desk_open (would send \(note.count) chars, \(ctx?.files.count ?? 0) files)") }
        let desk: String
        do { desk = try await store.resolveDesk() } catch { return finish(hint: "\(error)") } // off the hub: names the hub
        guard g == gesture, case .sending = state else { return }
        self.desk = desk
        let body = ([Self.noteLine(note, durationMs: durationMs)] + (ctx?.lines ?? [])).joined(separator: "\n")
        let files = ctx?.files ?? []
        let running = store.sessions.first { $0.id == desk }?.running ?? false
        let clientId = UUID().uuidString
        do {
            if running { _ = try await store.client.steer(session: desk, text: body, id: clientId, files: files, delivery: nil) }
            else { _ = try await store.client.sendInput(session: desk, text: body, files: files, delivery: nil, id: clientId) }
        } catch {
            return finish(hint: "\(error)")
        }
        guard g == gesture, case .sending = state else { return }
        router.sent(on: desk)
        PointLog.write("point sent to desk \(desk) via \(running ? "steer" : "send_input") (\(note.count) chars, \(files.count) files, reason \(reason))")
        card.state.begin(question: note)
        pill.hide()
        card.show(near: NSEvent.mouseLocation, key: !AskFlags.automated)
        send(.panelDone)
    }

    /// 45 §4.4's marker, as `voice-note.ts:5-6`.
    static func noteLine(_ text: String, durationMs: UInt64) -> String {
        "[voice note, \(max(1, (durationMs + 999) / 1000)) s] \(text)"
    }

    /// The pill shows why for `hintMs`; the machine leaves `sending` without a send.
    private func finish(hint: String) {
        PointLog.write("point not sent: \(hint)")
        send(.cancel)
        pill.showHint(hint, near: NSEvent.mouseLocation)
    }

    static func runLoopTurn() async {
        await withCheckedContinuation { c in DispatchQueue.main.async { c.resume() } }
    }

    func screenContext(samples: [PointerSample], durationMs: UInt64, color: String = Palette.malachiteHex) async -> ScreenContextReply? {
        guard VoiceKeySettings.includeScreen else { return nil }
        let req = ScreenContextReq(samples: samples, color: color, sinceMs: durationMs, accessibility: VoiceKeySettings.accessibility)
        do {
            let r = try await store.client.screenContext(req)
            PointLog.write("screen_context: \(r.lines.count) lines, \(r.files.count) files")
            return r
        } catch {
            PointLog.write("screen_context failed: \(error)")
            pill.state.hint = "Screenshots off — sent without the screen"
            return nil
        }
    }

    // MARK: the companion's dictation (the Ask bar's path, `AskBarPanel.swift:247-292`, with its own `AskDictation`)

    private func enqueue(_ op: @escaping () async throws -> Void) {
        let prev = sendTask
        sendTask = Task {
            await prev?.value
            do { try await op() } catch { PointLog.write("dictation call failed: \(error)") }
        }
    }

    /// `dictation_prewarm` once, on the first gesture (not at launch — the Ask bar's rule).
    private func prewarm() {
        guard !prewarmed else { return }
        prewarmed = true
        Task { [store] in
            do { try await store.client.dictationPrewarm() } catch { PointLog.write("dictation_prewarm failed: \(error)") }
        }
    }

    func beginDictation() {
        _ = dictation.handle(.start)
        pendingChunks = []; stopWanted = false
        if !AskFlags.automated {
            Task { [weak self] in
                guard let self, let why = await self.recorder.start() else { return }
                self.refuse(why)
            }
        }
        enqueue { [weak self, store] in
            do {
                let id = try await store.client.dictationStart()
                guard let self else { return }
                _ = self.dictation.handle(.started(id: id))
                PointLog.write("dictation_start → \(id)")
                for c in self.pendingChunks { self.sendChunk(c, id: id) }
                self.pendingChunks = []
                if self.stopWanted { self.stopWanted = false; self.endDictation() }
            } catch {
                guard let self else { return }
                _ = self.dictation.handle(.startFailed("\(error)"))
                self.refuse(self.dictation.error ?? "\(error)")
            }
        }
    }

    /// A refused `dictation_start` or microphone: the gesture ends with the hint (`talking` → "Meeting running").
    private func refuse(_ why: String) {
        PointLog.write("dictation refused: \(why)")
        resumeFinal() // a Stop already waiting for the final text gets what there is
        switch state {
        case .armed, .holding, .tap1, .locked: send(.cancel)
        default: return
        }
        pill.showHint(Self.hint(for: why), near: NSEvent.mouseLocation)
    }

    /// One ~100 ms chunk from the microphone (or the probe's tone).
    func feed(_ chunk: [Int16]) {
        if let id = dictation.id { sendChunk(chunk, id: id) } else if dictation.phase == .starting { pendingChunks.append(chunk) }
    }

    private func sendChunk(_ chunk: [Int16], id: String) {
        enqueue { [store] in try await store.client.dictationAudio(id: id, pcm16: chunk) }
    }

    func endDictation() {
        guard let id = dictation.id else { stopWanted = dictation.phase == .starting; return }
        _ = dictation.handle(.stop)
        enqueue { [store] in try await store.client.dictationStop(id: id) }
    }

    /// `DictationFinished` (or `finalTextMs`) → the text so far.
    func finalText() async -> String {
        if dictation.isActive {
            await withCheckedContinuation { c in
                finalWaiter = c
                DispatchQueue.main.asyncAfter(deadline: .now() + Self.finalTextMs) { [weak self] in self?.resumeFinal() }
            }
        }
        return dictation.liveText
    }

    private func resumeFinal() {
        finalWaiter?.resume()
        finalWaiter = nil
    }

    // MARK: events

    private struct Wire: Decodable {
        struct Reason: Decodable { let reason: String; let message: String? }
        let type: String
        let id: String?
        let session: String?
        let text: String?
        let stopReason: String?
        let message: String?
        let utterance: Int?
        let isFinal: Bool?
        let levels: [Float]?
        let reason: Reason?
        let name: String?
        let seq: UInt32?
        let opus: String?
        let sink: String?
        enum CodingKeys: String, CodingKey { case type, id, session, text, stopReason, message, utterance, isFinal = "final", levels, reason, name, seq, opus, sink }
    }

    func observe(_ data: Data) {
        guard let w = try? TheuthJSON.decoder.decode(Wire.self, from: data) else { return }
        switch w.type {
        case "DictationStarted": if let id = w.id { _ = dictation.handle(.started(id: id)) }
        case "DictationText":
            guard let id = w.id else { return }
            if dictation.handle(.text(id: id, utterance: w.utterance ?? 0, text: w.text ?? "", final: w.isFinal ?? false)) {
                pill.state.text = dictation.finalText
                pill.state.partial = dictation.partialText
                publish()
            }
        case "DictationLevel":
            guard let id = w.id else { return }
            if dictation.handle(.level(id: id, levels: w.levels ?? [])) || id == dictation.id { pill.state.level = dictation.levels.max() ?? 0 }
        case "DictationFinished":
            guard let id = w.id, id == dictation.id else { return }
            _ = dictation.handle(.finished(id: id, error: w.reason?.reason == "error" ? (w.reason?.message ?? "dictation failed") : nil))
            pill.state.text = dictation.liveText; pill.state.partial = ""
            resumeFinal()
        case "SpeechStarted": if let id = w.id, let s = w.session { speech.started(id: id, session: s, sink: w.sink) }
        case "SpeechAudio": if let id = w.id, let o = w.opus { speech.audio(id: id, opusBase64: o) }
        case "SpeechFinished": if let id = w.id { speech.finish(id: id, ok: w.reason?.reason == "done") }
        case "TextDelta", "TurnFinished", "Error", "ToolCallPending", "ToolCallStarted":
            replyEvent(w)
        default:
            if ((try? MeetingEvent.decode(data)) ?? nil) != nil { refreshMeeting() } // a meeting event: the mic lease may have moved
        }
    }

    /// The desk's turn drives the card: "working…", tool names, the final text; then the spoken reply and `doneMs`.
    private func replyEvent(_ w: Wire) {
        guard let r = router.reply, !r.finished, let session = w.session, session == r.session else { return }
        switch w.type {
        case "TextDelta": router.observe(.textDelta(session: session, text: w.text ?? ""))
        case "TurnFinished": router.observe(.turnFinished(session: session, stopReason: w.stopReason ?? ""))
        case "Error": router.observe(.error(session: session, message: w.message ?? ""))
        default:
            if let n = w.name { card.state.status = "working · \(n)" }
            return
        }
        guard let reply = router.reply else { return }
        card.state.text = reply.text
        card.state.status = reply.finished ? (reply.failure.map { "couldn't reply — \($0)" } ?? "replied") : "working…"
        card.state.failed = reply.failure != nil
        guard reply.finished else { return }
        if reply.failure == nil, VoiceKeySettings.speak, !reply.text.isEmpty { speak(reply.text, on: session) } else { armDone() }
    }

    /// Revision 1 row 10: `speak(desk, text)`; the packets come back as `SpeechAudio` and `SpeechPlayer` plays them.
    private func speak(_ text: String, on session: String) {
        speech.expect(session: session)
        card.state.speaking = true
        Task { [weak self] in
            do {
                let id = try await self?.store.client.speak(session: session, text: text)
                self?.speakingId = id
                PointLog.write("speak → \(id ?? "nil")")
            } catch {
                PointLog.write("speak failed: \(error)")
                self?.card.state.speaking = false
                self?.armDone()
            }
        }
    }

    private func stopSpeaking() {
        speech.stop()
        card.state.speaking = false
        guard let id = speakingId else { return }
        speakingId = nil
        Task { [store] in try? await store.client.stopSpeaking(id: id) }
    }

    /// `doneMs` after the reply is over (text final, nothing speaking) → `Replying → Idle`, the card goes.
    private func armDone() {
        doneTimer?.invalidate()
        doneTimer = Timer.scheduledTimer(withTimeInterval: TimeInterval(PointMachine.doneMs) / 1000, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.send(.panelDone) }
        }
    }
}

/// `voiceKey.*` of 96 (`voice-key.ts:37-47`) in `Settings.defaults` (D19); the Settings › Voice key tab binds the same keys.
enum VoiceKeySettings {
    static let enabledKey = "voiceKeyEnabled", includeScreenKey = "voiceKeyIncludeScreen", accessibilityKey = "voiceKeyAccessibility",
               annotateMouseKey = "voiceKeyAnnotateMouse", speakKey = "voiceKeySpeak"
    static var enabled: Bool { flag(enabledKey, true) }
    static var includeScreen: Bool { flag(includeScreenKey, true) }
    /// 96's `accessibility: 'auto' | 'off'` — `true` = auto (the engine's grant decides).
    static var accessibility: Bool { flag(accessibilityKey, true) }
    /// 96's `annotate: 'always' | 'mouse'`.
    static var annotateMouse: Bool { flag(annotateMouseKey, false) }
    /// Revision 1 row 10: on, as 96 shipped (a voice send always entered voice mode, `voice-key.ts:358`).
    static var speak: Bool { flag(speakKey, true) }

    private static func flag(_ key: String, _ def: Bool) -> Bool {
        Settings.defaults.object(forKey: key) == nil ? def : Settings.defaults.bool(forKey: key)
    }
}

extension Palette {
    /// The trail colour the engine stamps on its capture (`screen_context.color`), the dark malachite.
    static let malachiteHex = "#4a8a6e"
}

/// stderr plus `/tmp/149-W2-probe.log` during `--probe-point`.
enum PointLog {
    static let file = URL(fileURLWithPath: "/tmp/149-W2-probe.log")

    static func write(_ s: String) {
        Log.write(s)
        guard Flags.probePoint != nil else { return }
        let line = "[companion \(Date()) pid \(getpid())] \(s)\n"
        if let h = try? FileHandle(forWritingTo: file) { h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close() }
        else { try? Data(line.utf8).write(to: file) }
    }
}
