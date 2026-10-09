import AppKit
import Combine
import CompanionCore
import TheuthKit

/// 149 W1: hold-to-talk into a field (§3.2 per-row mic, §3.3 V on the card; §5.4 A). `AskVoiceMachine` + `VoiceRecorder`
/// + the engine's dictation stream, exactly the Ask bar's path (`AskBarPanel.swift`), with the text handed to `onText`
/// instead of theuth's desk. One instance per surface; the engine allows one dictation at a time ("dictating elsewhere").
/// Probes never reach `press` (`AskFlags.automated`): no microphone, no permission dialog.
@MainActor
final class HoldToTalk: ObservableObject {
    @Published private(set) var recording = false
    /// `dictation_stop` sent, the tail still flushing.
    @Published private(set) var finishing = false
    /// "listening…" · "finishing…" · a refusal ("dictating elsewhere", "Meeting running — use the island to ask").
    @Published private(set) var status: String?
    /// Finals + the open partial while recording.
    @Published private(set) var liveText = ""
    @Published private(set) var partial = ""
    /// `DictationLevel.levels`, newest right.
    @Published private(set) var levels: [Float] = []
    /// Set while a hold runs: what the text is for (a session id, a card id) — the caller's bookkeeping.
    var target: String?
    /// The dictated text, trimmed, non-empty.
    var onText: ((String) -> Void)?
    /// 153 §7.4 "send it": the text should go out without the caller's hold; `nil` falls back to `onText`.
    var onSend: ((String) -> Void)?

    private let store: Store
    private var machine = AskVoiceMachine()
    private let recorder = VoiceRecorder()
    private var timer: Timer?
    private var subs = Set<AnyCancellable>()
    private var sendTask: Task<Void, Never>?
    private var pending: [[Int16]] = []
    private var stopWanted = false
    /// §8.5: the recorder is open; chunks before the id wait in `pending`.
    private var micOpen = false
    /// 149g: as `AskBarController.hold` — the start's reply and refusal belong to the hold that sent them.
    private var hold = 0

    init(store: Store) {
        self.store = store
        recorder.onChunk = { [weak self] c in self?.feed(c) }
        store.$ask.map(\.dictation).removeDuplicates().receive(on: RunLoop.main)
            .sink { [weak self] d in MainActor.assumeIsolated { self?.dictationChanged(d) } }.store(in: &subs)
    }

    var isBusy: Bool { machine.isBusy }

    /// Key-down / pointer-down on the mic. A press under `AskVoiceMachine.holdThreshold` is a tap and does nothing — with
    /// "Open the mic on key-down" (153 §8.5) the recorder opens now and a tap discards it.
    func press(target: String? = nil) {
        guard !AskFlags.automated, !machine.isBusy else { return }
        self.target = target
        machine.micOnKeyDown = ModeKeySettings.micOnKeyDown(Settings.defaults)
        event(.keyDown(Date()))
    }

    func release() { event(.keyUp(Date())) }

    /// ⎋ or the surface closing: nothing lands.
    func cancel() {
        guard machine.isBusy else { return }
        apply(machine.handle(.failed("cancelled")))
        status = nil
    }

    private func event(_ e: AskVoiceMachine.Event) {
        apply(machine.handle(e))
        if machine.isBusy, timer == nil {
            timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.tick() }
            }
        } else if !machine.isBusy {
            timer?.invalidate(); timer = nil
        }
    }

    private func tick() {
        apply(machine.handle(.tick(Date())))
        recording = machine.isRecording
        if !machine.isBusy { timer?.invalidate(); timer = nil }
    }

    private func apply(_ effects: [AskVoiceMachine.Effect]) {
        for fx in effects {
            switch fx {
            case .tap:
                break
            case .openMic:
                startMic()
            case .discardMic:
                micOpen = false
                recorder.cancel()
                pending = []
            case .startRecording:
                recording = true
                status = "listening…"
                begin()
                if !micOpen { startMic() }
            case .stopDictation:
                recording = false
                finishing = true
                status = "finishing…"
                micOpen = false
                recorder.stop()
                end()
            case .setText(let text):
                landed()
                onText?(text)
            case .sendNow(let text):
                landed()
                (onSend ?? onText)?(text)
            case .fail(let why):
                recording = false
                finishing = false
                micOpen = false
                recorder.cancel()
                if let id = store.ask.dictation.id { enqueue { [store] in try await store.client.dictationStop(id: id) } }
                store.ask.dictation = AskDictation() // 149g: a start still in flight lands into a reset session and stops itself
                pending = []; stopWanted = false
                liveText = ""; partial = ""; levels = []
                status = why == "cancelled" ? nil : why
                Log.write("hold-to-talk: \(why)")
            }
        }
    }

    private func landed() {
        recording = false
        finishing = false
        status = nil
        micOpen = false
        recorder.cancel()
        liveText = ""; partial = ""; levels = []
    }

    private func startMic() {
        micOpen = true
        let g = hold
        Task { [weak self] in
            guard let self, let why = await self.recorder.start(), g == self.hold else { return }
            self.event(.failed(why))
        }
    }

    // MARK: the engine's dictation (54 §8) — mirrors `AskBarController.beginDictation/feed/endDictation`

    private func enqueue(_ op: @escaping () async throws -> Void) {
        let prev = sendTask
        sendTask = Task {
            await prev?.value
            do { try await op() } catch { Log.write("dictation call failed: \(error)") }
        }
    }

    private func begin() {
        _ = store.ask.dictation.handle(.start)
        stopWanted = false
        hold += 1
        let g = hold
        let pressed = machine.pressedAt
        enqueue { [weak self, store] in
            do {
                let ago = pressed.map { UInt32(max(0, Date().timeIntervalSince($0)) * 1000) }
                let id = try await store.client.dictationStart(opts: DictationOpts(surface: .companion, destination: .agentPrompt, pressAgoMs: ago))
                guard let self else { return }
                guard store.ask.dictation.awaitsStart(from: g, current: self.hold) else {
                    Log.write("hold-to-talk: dictation_start → \(id) landed after the hold ended: dictation_stop")
                    return try await store.client.dictationStop(id: id)
                }
                _ = store.ask.dictation.handle(.started(id: id))
                for c in self.pending { self.send(c, id: id) }
                self.pending = []
                if self.stopWanted { self.stopWanted = false; self.end() }
            } catch {
                guard let self else { return }
                Log.write("hold-to-talk: dictation_start failed: \(error)")
                guard store.ask.dictation.awaitsStart(from: g, current: self.hold) else { return }
                _ = store.ask.dictation.handle(.startFailed("\(error)"))
                let why = store.ask.dictation.error ?? "\(error)"
                self.event(.failed(why))
            }
        }
    }

    private func feed(_ chunk: [Int16]) {
        if let id = store.ask.dictation.id { send(chunk, id: id) } else if store.ask.dictation.phase == .starting || micOpen { pending.append(chunk) }
    }

    private func send(_ chunk: [Int16], id: String) {
        enqueue { [store] in try await store.client.dictationAudio(id: id, pcm16: chunk) }
    }

    private func end() {
        guard let id = store.ask.dictation.id else { stopWanted = store.ask.dictation.phase == .starting; return }
        _ = store.ask.dictation.handle(.stop)
        enqueue { [store] in try await store.client.dictationStop(id: id) }
        // ponytail: the Ask bar's 5 s cap on "finishing…" — the text so far lands if DictationFinished never comes.
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) { [weak self] in
            guard let self, case .stopping = self.machine.state else { return }
            self.event(.dictated(self.store.ask.dictation.liveText))
        }
    }

    private func dictationChanged(_ d: AskDictation) {
        guard machine.isBusy, !machine.isPressed else { return }
        liveText = d.finalText
        partial = d.partialText
        levels = d.levels
        if d.isActive, let h = d.deviceHint { status = h } else if d.isActive, d.send { status = "sending…" }
        // 153 §7.3: the engine may end the dictation itself ("send it" settled); a finish while recording lands like a release.
        if !d.isActive { event(d.error.map { .failed($0) } ?? .dictated(d.liveText, send: d.send)) }
    }
}

/// 149 W1 (§3.1/§3.2): hover shared between the rail and the agents panel — a hovered dot grows and highlights its row,
/// a hovered row grows its dot. Keyed by session id (`Mark.session` ↔ `AgentRow.session`).
@MainActor
final class HoverSync: ObservableObject {
    @Published var session: String?
}
