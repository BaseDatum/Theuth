import AppKit
import Combine
import CompanionCore
import KeyboardShortcuts
import TheuthKit

extension KeyboardShortcuts.Name {
    /// The voice key (`KeyAction.voiceKey`, ⌃S on macOS): the chord comes from the table, never a library default.
    static let voiceKey = Self("voiceKey")
    /// 153 §15: the command key (`KeyAction.command`, ⌃⌥C) — the same gesture with `mode = .command`.
    static let commandKey = Self("commandKey")
    /// ⎋ between press and send only (§8's pointAndSay row; `DESK/point/mod.rs:38-42`'s rule) — enabled/disabled per gesture.
    static let pointEscape = Self("pointEscape", default: .init(.escape))
    /// 149g: the same ⎋ carrying the voice key's modifiers (⌃⎋ for ⌃S) — what ⎋ is while the chord is physically held, which
    /// plain ⎋ never matches. Set from the table (`KeyTable.escapeDuringHold`), enabled/disabled with `pointEscape`.
    static let pointEscapeHeld = Self("pointEscapeHeld")
    /// 153 §15: the held-⎋ twin for the command chord (⌃⌥⎋).
    static let commandEscapeHeld = Self("commandEscapeHeld")
    /// 153 §15 / D2: the dictate-anywhere key (`KeyAction.dictateAnywhere`, ⌃⌥D) — the same gesture with `mode = .dictate`.
    static let dictateKey = Self("dictateKey")
    static let dictateEscapeHeld = Self("dictateEscapeHeld")
}

@MainActor private var pointAndSay: PointController?

extension AppDelegate {
    /// 149 W2: the voice key, overlays, dictation, screen context, the answer card, the spoken reply; `--probe-point`.
    /// 153 C1: the command key, `--probe-command`, `--probe-card`. 153 D2: the dictate key, `--probe-anyapp`.
    func installPointAndSay() {
        let c = PointController(store: store)
        pointAndSay = c
        if let seconds = Flags.probePoint { PointProbe.run(seconds: seconds, store: store, controller: c) }
        if let kind = Flags.probeCommand { CommandProbe.run(kind: kind, card: false, store: store, controller: c) }
        if let kind = Flags.probeCard { CommandProbe.run(kind: kind, card: true, store: store, controller: c) }
        if let kind = Flags.probeAnyApp { AnyAppProbe.run(kind: kind, store: store, controller: c) }
    }
}

extension Store {
    /// Every raw event (after `askObserve`): `Dictation*` for the voice key's own session, the desk's reply, `Speech*`.
    func pointObserve(_ data: Data) { pointAndSay?.observe(data) }
    /// 153 §12.1: the Ask bar hands its reply state to the one card (`Store.showAnswerCard`).
    func pointShowCard(_ state: AnswerCardState) { pointAndSay?.showExternal(state) }
}

extension Flags {
    /// `--probe-point N`: the spiral on every display, the Stop sequence with a synthetic tone and a real `screen_context`,
    /// quit after N s; nothing sent (§10).
    static let probePoint: TimeInterval? = {
        guard let i = args.firstIndex(of: "--probe-point"), i + 1 < args.count else { return nil }
        return TimeInterval(args[i + 1])
    }()
    /// 153 §16 row 20: `--probe-command <heard|working|approval|done>` / `--probe-card <collapsed|expanded|failed>` → PNGs.
    static let probeCommand: String? = {
        guard let i = args.firstIndex(of: "--probe-command"), i + 1 < args.count else { return nil }
        return args[i + 1]
    }()
    static let probeCard: String? = {
        guard let i = args.firstIndex(of: "--probe-card"), i + 1 < args.count else { return nil }
        return args[i + 1]
    }()
    /// 153 §16 row 24: `--probe-anyapp <refused|landed|notlanded|rewrite>` → PNGs under `/tmp/153-d2/` (render only).
    static let probeAnyApp: String? = {
        guard let i = args.firstIndex(of: "--probe-anyapp"), i + 1 < args.count else { return nil }
        return args[i + 1]
    }()
}

/// 149 §3.7 / §5.4 D: the driver around `PointMachine` — the hotkey's key-down/key-up and ⎋ in, the effects out. Owns the
/// timers (`after`), the sampler, the overlays, the voice pill, its own dictation session (`AskDictation`, the Ask bar's
/// reducer with the companion's PCM — D17/Revision 1 row 11), the Stop sequence and the answer card.
/// 153 §10: one controller for both mode keys — `mode` is chosen by the key that pressed and relabelled by the other key
/// while the gesture is live; `.command` sends `[command, N s]` and reads the desk's turn into the step-typed card.
@MainActor
final class PointController {
    /// 96's `FINAL_TEXT_MS`: how long the final flush may take after `dictation_stop` before what we have is sent.
    static let finalTextMs: TimeInterval = 8

    private let store: Store
    private(set) var state = PointMachine.State.idle
    private var pressAt: UInt64 = 0
    /// Bumped on every `Start`: a Stop sequence that outlives its gesture drops its result. The probe counts gestures by it.
    private(set) var gesture = 0
    /// 153 §10.1: the live gesture's mode (chosen at press, switched by the other key); the card's thread follows it.
    private(set) var mode = PointMachine.Mode.ask
    /// §12.6: the gesture table chosen at press (per mode).
    private var gestureKind = PointMachine.Gesture.holdLock
    /// The press landed while the card was up: the send is a follow-up in its thread when the mode matches (§10.5 / §12.2);
    /// the card steps aside for the pill (both live in the island area) and comes back at the send or a discard.
    private var cardWasUp = false
    private var followUp: Bool { cardWasUp && card.state.kind == cardKind && card.state.session != nil }
    let sampler = PointerSampler()
    let overlays = OverlayWindows()
    let pill = VoicePill()
    let card = AnswerCard()
    /// 153 D2: the target side of `.dictate` — AX capture, delivery, verification, the failure card, rewrite.
    let anyApp: AnyAppController
    /// 149s: the Store's shared `Speaker`; `card.state.speaking` mirrors its `surface == .voiceKey`.
    private var speaker: Speaker { store.speaker }
    private let recorder = VoiceRecorder()
    private(set) var dictation = AskDictation()
    private var router = AskRouter()
    private var displays: [OverlayDisplay] = []
    private var subs = Set<AnyCancellable>()
    /// 189 D2: the voice key's reply watchdog (`AskReply.watchdogDeadline`).
    private let watchdog = ReplyWatchdog()
    private var sendTask: Task<Void, Never>?
    private var pendingChunks: [[Int16]] = []
    private var stopWanted = false
    private var finalWaiter: CheckedContinuation<Void, Never>?
    private var doneTimer: Timer?
    private var wasSpeaking = false
    private var desk: String?
    /// The last `meeting_status` seen: a live meeting leases the mic, so a press is refused with the "Meeting running" hint.
    private var meetingLive = false
    private var prewarmed = false
    /// §10.6: the spoken-approval listen is live (`dictation` is its session); the timer closes it after `listenSecs`.
    private var approvalListen: Timer?
    /// `--probe-point`'s gesture phase: the Stop sequence runs to the point of `desk_open` and stops there.
    var dryRun = false
    /// `--probe-point` only: `dictation_start` is held this long before it is sent, so a release-into-send that beats the
    /// reply (a slow engine) is reproducible against a live one.
    var probeStartDelay: Duration = .zero
    /// 149f: one physical press = one gesture; auto-repeat key-downs never reach the machine.
    private var gate = VoiceKeyGate()

    init(store: Store) {
        self.store = store
        anyApp = AnyAppController(store: store)
        // A probe registers nothing: a Carbon hot key swallows the chord for every app while the process lives.
        if !AskFlags.automated {
            applyHotkey(SettingsModel.shared.value)
            SettingsModel.shared.$value.removeDuplicates().dropFirst().receive(on: RunLoop.main)
                .sink { [weak self] v in MainActor.assumeIsolated { self?.applyHotkey(v) } }
                .store(in: &subs)
            KeyboardShortcuts.onKeyDown(for: .voiceKey) { [weak self] in self?.keyDown(.ask) }
            KeyboardShortcuts.onKeyUp(for: .voiceKey) { [weak self] in self?.keyUp(.ask) }
            KeyboardShortcuts.onKeyDown(for: .commandKey) { [weak self] in self?.keyDown(.command) }
            KeyboardShortcuts.onKeyUp(for: .commandKey) { [weak self] in self?.keyUp(.command) }
            KeyboardShortcuts.onKeyDown(for: .dictateKey) { [weak self] in self?.keyDown(.dictate) }
            KeyboardShortcuts.onKeyUp(for: .dictateKey) { [weak self] in self?.keyUp(.dictate) }
            for name in [KeyboardShortcuts.Name.pointEscape, .pointEscapeHeld, .commandEscapeHeld, .dictateEscapeHeld] {
                KeyboardShortcuts.onKeyDown(for: name) { [weak self] in self?.send(.esc) }
            }
            escapeHotkeys(false)
        }
        recorder.onChunk = { [weak self] c in self?.feed(c) }
        anyApp.onLabel = { [weak self] l in self?.pill.state.label = l }
        anyApp.onHint = { [weak self] h in self?.pill.showHint(h, near: NSEvent.mouseLocation) }
        card.onEscape = { [weak self] in self?.cardEscape() }
        card.onOpen = { [weak self] in
            guard let self, let d = self.card.state.session ?? self.desk else { return }
            self.store.onOpenURL?(DeepLink.session(d))
        }
        card.onStop = { [weak self] in self?.speaker.stop() }
        card.onHide = { [weak self] in self?.store.askCardHidden() } // 189 D1: the Ask's focus claim ends with its card
        card.model.onApprove = { [weak self] d in self?.resolveApproval(d, spoken: false) }
        card.model.onFollowUp = { [weak self] t in self?.typedFollowUp(t) }
        card.model.onExpandChanged = { [weak self] expanded in
            guard let self else { return }
            if expanded { self.doneTimer?.invalidate(); self.doneTimer = nil } else { self.armDoneIfReady() }
        }
        card.historySource = { [weak self] in self?.store.ask.receipts.entries ?? [] }
        speaker.$surface.removeDuplicates().receive(on: RunLoop.main)
            .sink { [weak self] s in MainActor.assumeIsolated { self?.speakingChanged(s == .voiceKey) } }
            .store(in: &subs)
        // Installed before `store.start` (the insertion point); the client exists once the connection opens.
        // 189 D2: a reconnect may have lost the reply's events — read the transcript.
        store.$connection.removeDuplicates().receive(on: RunLoop.main)
            .sink { [weak self] s in MainActor.assumeIsolated { if s == .open { self?.refreshMeeting(); self?.catchUp(.reconnect) } } }
            .store(in: &subs)
    }

    /// The table's `voiceKey` / `command` chords into KeyboardShortcuts; off = unregistered. NOT in `Hotkeys.names`: that
    /// launch probe would see Tauri's ⌃S (still bound until `companion-alive` lands, D22) and persist the row as unassigned.
    private func applyHotkey(_ v: CompanionSettings) {
        guard !AskFlags.automated else { return } // a probe registers nothing (the Settings tabs call `hotkeyChanged` on appear)
        var heldSeen: [KeyboardShortcuts.Shortcut] = []
        let rows: [(KeyAction, Bool, KeyboardShortcuts.Name, KeyboardShortcuts.Name)] = [
            (.voiceKey, VoiceKeySettings.enabled, .voiceKey, .pointEscapeHeld),
            (.command, ModeKeySettings.commandKeyEnabled(Settings.defaults), .commandKey, .commandEscapeHeld),
            (.dictateAnywhere, AnyAppController.hotkeyEnabled, .dictateKey, .dictateEscapeHeld),
        ]
        for (action, enabled, name, heldName) in rows {
            let chord = enabled ? v.keys.chord(for: action) : nil
            let want = chord.map(Hotkeys.shortcut)
            if KeyboardShortcuts.getShortcut(for: name) != want {
                KeyboardShortcuts.setShortcut(want, for: name)
                PointLog.write("hotkey \(name.rawValue) \(want.map { "registered \($0)" } ?? "unregistered")")
            }
            // 149g: ⎋ as the held chord makes it; `setShortcut` registers at once, so it goes off again unless a gesture is live.
            // The held-⎋ twin is one Carbon registration per chord: ⌃⌥D and ⌃⌥C share ⌃⌥⎋, so the second row leaves it to the first.
            var held = chord.flatMap { KeyTable.escapeDuringHold(of: $0, reserved: Hotkeys.reserved) }.map(Hotkeys.shortcut)
            if let h = held { if heldSeen.contains(h) { held = nil } else { heldSeen.append(h) } }
            guard KeyboardShortcuts.getShortcut(for: heldName) != held else { continue }
            KeyboardShortcuts.setShortcut(held, for: heldName)
            switch state {
            case .idle, .replying: KeyboardShortcuts.disable(heldName)
            default: break
            }
            PointLog.write("hotkey \(heldName.rawValue) \(held.map { "set \($0)" } ?? "cleared — \(chord?.symbol ?? "no chord"): ⎋ with its modifiers is bare or the system's")")
        }
    }

    /// The ⎋ registrations live exactly as long as a gesture (press → send/discard); logged as the voice key's are.
    private func escapeHotkeys(_ on: Bool) {
        guard !AskFlags.automated else { return } // a probe registers nothing, ⎋ included
        let names: [KeyboardShortcuts.Name] = [.pointEscape, .pointEscapeHeld, .commandEscapeHeld, .dictateEscapeHeld]
        for name in names { on ? KeyboardShortcuts.enable(name) : KeyboardShortcuts.disable(name) }
        PointLog.write("hotkey pointEscape \(on ? "registered" : "unregistered") \(names.compactMap(KeyboardShortcuts.getShortcut).map { "\($0)" }.joined(separator: " "))")
    }

    /// Settings › Voice key / › Command flipped "enabled".
    static func hotkeyChanged() { pointAndSay?.applyHotkey(SettingsModel.shared.value) }

    private func chordSymbol(_ m: PointMachine.Mode) -> String {
        let keys = SettingsModel.shared.value.keys
        switch m {
        case .command: return keys.chord(for: .command)?.symbol ?? "⌃⌥C"
        case .dictate: return keys.chord(for: .dictateAnywhere)?.symbol ?? "⌃⌥D"
        case .ask: return keys.chord(for: .voiceKey)?.symbol ?? "⌃S"
        }
    }

    /// The pill's label after the chord; `.dictate` names the target ("Dictating into Mail") once the press assessed it.
    private func pillLabel(_ m: PointMachine.Mode) -> String {
        switch m {
        case .command: return "Command · theuth desk"
        case .dictate: return anyApp.session?.label ?? "Dictate anywhere"
        case .ask: return "theuth's desk"
        }
    }

    private func refreshMeeting() {
        Task { [weak self] in
            guard let self, let live = try? await self.store.client.meetingStatus().state.isLive else { return }
            self.meetingLive = live
        }
    }

    // MARK: the machine

    /// The hot key's `kEventHotKeyPressed` — Carbon re-posts it for every keyboard auto-repeat while the chord is held, and
    /// KeyboardShortcuts forwards each one — filtered by `VoiceKeyGate` to the real key-down; the probe's synthetic presses
    /// come in here too. A gesture a refusal ended stays ended until the key is released. 153 §10.1: the other mode's key
    /// while a gesture is live switches the mode (its auto-repeats are filtered by the same gate: the mode already matches).
    func keyDown(_ m: PointMachine.Mode = .ask) {
        if isLive, m != mode { send(.switchMode(m)); return }
        guard gate.press(now: PointClock.now()) else { return }
        if !isLive {
            mode = m
            gestureKind = ModeKeySettings.gesture(m, Settings.defaults)
        }
        send(.pressed)
    }

    /// Either chord's release ends the hold (§10.1); the second chord's key-up finds the gate already up and is ignored.
    func keyUp(_ m: PointMachine.Mode = .ask) {
        guard gate.release() else { return PointLog.write("hotkey key-up without a key-down (ignored)") }
        send(.released)
    }

    /// Press → send/discard: the states in which the other mode's key switches the mode.
    private var isLive: Bool {
        switch state {
        case .idle, .sending, .replying: return false
        default: return true
        }
    }

    func send(_ e: PointMachine.Event) {
        let now = PointClock.now()
        let refused: String? = (meetingLive || store.meeting?.isLive == true) ? "talking" : nil
        let (next, effects) = PointMachine.step(state, e, now: now, refused: refused, gesture: gestureKind)
        let prev = state
        state = next
        if prev != next || !effects.isEmpty { PointLog.write("point \(e) \(prev) → \(next) \(effects)") }
        publish()
        for fx in effects { run(fx, now: now) }
        after(prev, next)
    }

    private func publish() {
        store.point = state == .idle ? nil : PointSlice(state: state, samples: sampler.samples, text: dictation.liveText)
    }

    private var cardKind: AnswerCardState.Kind { mode == .command ? .command : .voiceNote }

    private func run(_ fx: PointMachine.Effect, now: UInt64) {
        switch fx {
        case .start:
            gesture += 1
            pressAt = now
            endApprovalListen()
            // §10.5 / §12.2: a hold while the card is up in the same mode is a follow-up in its thread; else the old reply is over.
            cardWasUp = card.isVisible
            card.hide(returnKey: false)
            doneTimer?.invalidate(); doneTimer = nil
            speaker.stop() // S3: a new gesture silences whatever is speaking, the Ask bar's reply included
            resetReply()
            // D2 §13.1: the target is assessed before the mic opens; a certainly-not target ends the gesture with the hint.
            if mode == .dictate, !anyAppPress() { return }
            if mode != .dictate { // the pointer trail is the voice key's and the command key's (§3.2: the pill alone for dictation)
                displays = PointerSampler.displays()
                let mouse = VoiceKeySettings.annotateMouse
                overlays.reveal(catchMouse: mouse)
                sampler.onTick = { [weak self] s, _ in self?.tick(s) }
                sampler.start(t0: now, penAlways: !mouse, displays: displays)
            }
            pill.state.reset(chord: chordSymbol(mode), label: pillLabel(mode))
            pill.show(near: NSEvent.mouseLocation)
            escapeHotkeys(true)
            prewarm()
            beginDictation(opts: mode == .dictate ? anyApp.opts(pressAgoMs: 0) : nil)
        case .showLock:
            pill.state.phase = .locked
        case .relabel(let m):
            let was = mode
            mode = m
            if was == .dictate, m != .dictate { anyApp.abandon() }
            if m == .dictate, was != .dictate, !anyAppPress() { return }
            pill.state.chord = chordSymbol(m); pill.state.label = pillLabel(m)
            PointLog.write("point mode → \(m)")
        case .stop(let reason):
            let g = gesture
            Task { await self.stopSequence(gesture: g, reason: reason) }
        case .discard(let reason, let hint):
            if mode == .dictate { anyApp.abandon() }
            teardownGesture()
            if let hint { pill.showHint(hint, near: NSEvent.mouseLocation) } else { pill.hide() }
            if followUp { card.show(near: NSEvent.mouseLocation, key: !AskFlags.automated) } // the thread it interrupted stays
            cardWasUp = false
            PointLog.write("point discarded: \(reason)")
        case .conceal:
            card.hide(returnKey: true)
            pill.hide()
            doneTimer?.invalidate(); doneTimer = nil
            endApprovalListen()
            stopSpeaking()
            resetReply()
        case .refuse(let reason):
            pill.showHint(Self.hint(for: reason), near: NSEvent.mouseLocation)
            PointLog.write("point refused: \(reason)")
        }
    }

    /// Timers and the ⎋ lifetime follow the state, not the effects (`DESK/point/mod.rs:342-353`).
    private func after(_ prev: PointMachine.State, _ next: PointMachine.State) {
        switch next {
        case .armed(let t0): armTick(at: t0 + (gestureKind == .automatic ? PointMachine.tapToggleMs : PointMachine.holdMs))
        case .tap1(_, let released): armTick(at: released + PointMachine.doubleTapMs + 1)
        case .idle, .replying:
            switch prev {
            case .idle, .replying: break
            default: escapeHotkeys(false); refreshMeeting()
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

    /// Every pointer sample redraws the trail; the pill stays in the island area (149i §P).
    private func tick(_ s: PointerSample?) {
        if s != nil { overlays.draw(samples: sampler.samples, displays: displays) }
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
        if mode == .dictate { return await dictateStop(gesture: g, reason: reason) }
        let samples = sampler.stop()
        let durationMs = PointClock.now() &- pressAt
        // D13: every overlay out and one run-loop turn awaited before the capture can start.
        overlays.conceal()
        await Self.runLoopTurn()
        recorder.stop() // the tail chunk goes out through `feed`
        endDictation()
        pill.state.phase = .sending
        let mode = self.mode
        async let text = finalText()
        async let context = Self.wantsScreen(mode) ? screenContext(samples: samples, durationMs: durationMs) : nil
        let (t, ctx) = await (text, context)
        guard g == gesture, case .sending = state else { return }
        let note = t.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !note.isEmpty else { return finish(hint: "Nothing recognized") }
        if dryRun { return finish(hint: "probe — stopped before desk_open (would send \(note.count) chars, \(ctx?.files.count ?? 0) files)") }
        let desk: String
        do { desk = try await store.resolveDesk() } catch { return finish(hint: "\(error)") } // off the hub: names the hub
        guard g == gesture, case .sending = state else { return }
        self.desk = desk
        let kind = mode == .command ? AnswerCardState.Kind.command : .voiceNote
        let body = ([AnswerCardState.marker(kind, durationMs: durationMs) + note] + (ctx?.lines ?? [])).joined(separator: "\n")
        do { try await deliver(body, files: ctx?.files ?? [], to: desk) } catch { return finish(hint: "\(error)") }
        guard g == gesture, case .sending = state else { return }
        PointLog.write("point sent to desk \(desk) (\(note.count) chars, \(ctx?.files.count ?? 0) files, reason \(reason), mode \(mode), follow-up \(followUp))")
        if followUp { card.state.followUp(question: note) } else { card.state.begin(question: note, kind: kind) }
        cardWasUp = false
        card.state.session = desk
        card.state.status = mode == .command ? "heard → working" : "working…"
        pill.hide()
        card.show(near: NSEvent.mouseLocation, key: !AskFlags.automated)
        send(.panelDone)
    }

    // MARK: dictate anywhere (153 D2, §3.2 / §13)

    /// The press side of `.dictate`: the AX assessment; a refusal ends the gesture with the §13.1 hint before the mic opens.
    /// Answers whether the gesture goes on.
    private func anyAppPress() -> Bool {
        let (refused, _) = anyApp.press()
        guard let refused else { return true }
        Task { @MainActor in
            self.send(.cancel)
            self.pill.showHint(refused, near: NSEvent.mouseLocation)
        }
        return false
    }

    /// The gesture ended (release / tap / "send it" settled): the open utterance is flushed and delivered, "send it" presses
    /// Return when §7.4 allows, the pill says "✓ N lines into ‹App›" or the failure card opens (§13.5). Rewrite mode: the
    /// words are the instruction (§13.6). Works from `sending` (a release) and from a live state ("send it" ended the
    /// dictation under the held key): `finish(hint:)`'s `.cancel` idles the machine either way.
    private func dictateStop(gesture g: Int, reason: String) async {
        let mouse = NSEvent.mouseLocation
        recorder.stop()
        endDictation()
        pill.state.phase = .sending
        let text = await finalText()
        guard g == gesture, mode == .dictate, state != .idle else { return }
        let sendIt = dictation.send
        PointLog.write("anyapp stop (\(reason)): \(dictation.utterances.count) utterances, send it \(sendIt)")
        let hint = await anyApp.finish(text: text, sendIt: sendIt, mouse: mouse)
        guard g == gesture else { return }
        send(.cancel)
        if !hint.isEmpty { pill.showHint(hint, near: mouse) } else { pill.hide() }
    }

    /// §10.1 / Q (writer's default): the voice key sends the screen when its setting says so; command mode only when
    /// "Include the screen" is on in Settings › Command (default off).
    static func wantsScreen(_ m: PointMachine.Mode) -> Bool {
        m == .command ? CommandSettings.includeScreen : VoiceKeySettings.includeScreen
    }

    /// The desk's path: `steer` into a running turn, else `send_input` (`PointController.swift:287-297` of 149).
    /// 189 D1: the desk's focus is claimed before the send so its first deltas pass; a failed send with nothing else
    /// collecting withdraws it.
    private func deliver(_ body: String, files: [String], to desk: String) async throws {
        let running = store.sessions.first { $0.id == desk }?.running ?? false
        let clientId = UUID().uuidString
        store.claimFocus(.point, [desk])
        do {
            if running { _ = try await store.client.steer(session: desk, text: body, id: clientId, files: files, delivery: nil) }
            else { _ = try await store.client.sendInput(session: desk, text: body, files: files, delivery: nil, id: clientId) }
        } catch {
            if router.reply?.finished ?? true { store.claimFocus(.point, []) }
            throw error
        }
        router.sent(on: desk, question: body)
        armWatchdog()
        PointLog.write("delivered via \(running ? "steer" : "send_input")")
    }

    /// A new gesture or a concealed card: the reply being collected goes, with its focus claim and its watchdog.
    private func resetReply() {
        router = AskRouter()
        store.claimFocus(.point, [])
        armWatchdog()
    }

    /// 45 §4.4's marker, as `voice-note.ts:5-6`.
    static func noteLine(_ text: String, durationMs: UInt64) -> String {
        AnswerCardState.marker(.voiceNote, durationMs: durationMs) + text
    }

    /// §12.2: the expanded card's field — the same marker family, the same desk; the card appends a block.
    private func typedFollowUp(_ text: String) {
        guard let desk = card.state.session else { return }
        let kind = card.state.kind
        let body = AnswerCardState.marker(kind, durationMs: 0) + text
        doneTimer?.invalidate(); doneTimer = nil
        Task {
            do { try await deliver(body, files: [], to: desk) } catch { card.state.status = "couldn't send — \(error)"; return }
            self.desk = desk
            card.state.followUp(question: text)
            card.state.status = kind == .command ? "heard → working" : "working…"
        }
    }

    /// The pill shows why for `hintMs`; the machine leaves `sending` without a send.
    private func finish(hint: String) {
        PointLog.write("point not sent: \(hint)")
        send(.cancel)
        pill.showHint(hint, near: NSEvent.mouseLocation)
        if followUp { card.show(near: NSEvent.mouseLocation, key: !AskFlags.automated) }
        cardWasUp = false
    }

    static func runLoopTurn() async {
        await withCheckedContinuation { c in DispatchQueue.main.async { c.resume() } }
    }

    /// `color` nil = the accent, resolved at request time (in the body: a default argument is evaluated nonisolated).
    func screenContext(samples: [PointerSample], durationMs: UInt64, color: String? = nil) async -> ScreenContextReply? {
        let req = ScreenContextReq(samples: samples, color: color ?? Self.accentHex, sinceMs: durationMs, accessibility: VoiceKeySettings.accessibility)
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

    /// `opts` only for the §10.6 approval listen (`Surface.companion`); the gesture's own dictation is today's call.
    func beginDictation(opts: DictationOpts? = nil) {
        _ = dictation.handle(.start)
        pendingChunks = []; stopWanted = false
        let g = gesture
        if !AskFlags.automated {
            Task { [weak self] in
                guard let self, let why = await self.recorder.start(), g == self.gesture else { return }
                self.refuse(why)
            }
        }
        enqueue { [weak self, store] in
            do {
                if let d = self?.probeStartDelay, d > .zero { try? await Task.sleep(for: d) }
                let id = try await store.client.dictationStart(opts: opts)
                guard let self else { return }
                // The gesture ended (cancel/Esc/short) before the reply: the dictation is live on the engine with no owner —
                // stop it now, or it holds the one slot until `dictation_max_secs` ("dictating elsewhere" everywhere).
                guard self.dictation.awaitsStart(from: g, current: self.gesture) else {
                    PointLog.write("dictation_start → \(id) landed after the gesture ended: dictation_stop")
                    return try await store.client.dictationStop(id: id)
                }
                _ = self.dictation.handle(.started(id: id))
                PointLog.write("dictation_start → \(id)")
                for c in self.pendingChunks { self.sendChunk(c, id: id) }
                self.pendingChunks = []
                if self.stopWanted { self.stopWanted = false; self.endDictation() }
            } catch {
                guard let self else { return }
                PointLog.write("dictation_start failed: \(error)")
                guard self.dictation.awaitsStart(from: g, current: self.gesture) else { return }
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
        case .armed, .holding, .tap1, .locked, .toggled: send(.cancel)
        default: endApprovalListen(); return
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

    struct Wire: Decodable {
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
        let callId: String?
        /// `ToolCallStarted.args` (`crates/agent/src/lib.rs` `args: serde_json::Value`); `arguments` accepted too (C2's wording).
        let args: JSONValue?
        let arguments: JSONValue?
        let isError: Bool?
        let tool: String?
        let risk: String?
        let decision: String?
        let timedOut: Bool?
        /// 153 §4.4 / §7: the shaped form and the end command of a `DictationText` piece (D2's dictate mode reads them).
        let shaped: String?
        let command: EndCommand?
        /// 189 D2: `TurnStarted.turn_seq`, the transcript's turn id.
        let turnSeq: Int64?
        enum CodingKeys: String, CodingKey {
            case type, id, session, text, stopReason, message, utterance, isFinal = "final", levels, reason, name, callId, args, arguments, isError, tool, risk, decision, timedOut, shaped, command, turnSeq
        }
    }

    func observe(_ data: Data) {
        guard let w = try? TheuthJSON.decoder.decode(Wire.self, from: data) else { return }
        switch w.type {
        // `DictationStarted` is broadcast for EVERY dictation (the Ask bar's, Tauri's, a probe's): adopting it while `.starting`
        // took a foreign id as ours (149g: the REFUSED HOLD probe then stopped the probe's dictation). Our id is the
        // `dictation_start` reply's, nothing else.
        case "DictationText":
            guard let id = w.id else { return }
            // §4.4: the field form is `shaped ?? text` — what goes into the app; the voice key keeps sending the raw words.
            let text = mode == .dictate ? (w.shaped ?? w.text ?? "") : (w.text ?? "")
            let i = w.utterance ?? 0, final = w.isFinal ?? false
            if dictation.handle(.text(id: id, utterance: i, text: text, final: final, command: w.command)) {
                if approvalListen != nil { if final { spokenApproval(w.text ?? "") }; return }
                pill.state.text = dictation.finalText
                pill.state.partial = dictation.partialText
                publish()
                if mode == .dictate, final { anyApp.utterance(i, text: text, final: true) } // §3.2: delivered as it closes
            }
        case "DictationLevel":
            guard let id = w.id else { return }
            if dictation.handle(.level(id: id, levels: w.levels ?? [])) || id == dictation.id { pill.state.level = dictation.levels.max() ?? 0 }
        case "DictationFinished":
            guard let id = w.id, id == dictation.id else { return }
            _ = dictation.handle(.finished(id: id, error: w.reason?.reason == "error" ? (w.reason?.message ?? "dictation failed") : nil))
            if approvalListen != nil { return endApprovalListen() }
            pill.state.text = dictation.liveText; pill.state.partial = ""
            resumeFinal()
            // §7.3: "send it" settled and the engine ended the dictation under the held key — the gesture ends now.
            if mode == .dictate, dictation.send, isLive { let g = gesture; Task { await self.dictateStop(gesture: g, reason: "send it") } }
        case "SpeechStarted", "SpeechAudio", "SpeechFinished": break // 149s: the Store's SpeechPlayer takes them
        case "TextDelta", "TurnStarted", "TurnFinished", "Error", "ToolCallPending", "ToolCallStarted", "ToolCallFinished":
            replyEvent(w)
        case "Lagged":
            catchUp(.lagged)
        case "ApprovalRequested", "ApprovalResolved":
            approvalEvent(w, data: data)
        default:
            if ((try? MeetingEvent.decode(data)) ?? nil) != nil { refreshMeeting() } // a meeting event: the mic lease may have moved
        }
    }

    /// The desk's turn drives the card: "working…", tool names, the final text; then the spoken reply and `doneMs`.
    /// Command mode (§10.3): the same events feed the `CommandCard` reducer — rows from `ToolCallStarted` with `purpose`,
    /// colours from `ToolCallFinished`, the ✓/✗ line when the reply finishes, the cap at 12. 189 D2: a finished turn reads
    /// the transcript before the reply ends (`catchUp`).
    private func replyEvent(_ w: Wire) {
        guard let r = router.reply, !r.finished, let session = w.session, session == r.session else { return }
        defer { armWatchdog() }
        switch w.type {
        case "TextDelta":
            if router.observe(.textDelta(session: session, text: w.text ?? "")) { card.state.command?.observe(.textDelta(w.text ?? "")) }
            if router.reply?.deltas == 1 { PointLog.write("point: first TextDelta from \(session.prefix(8))") }
        case "TurnStarted":
            router.observe(.turnStarted(session: session, turnSeq: w.turnSeq, text: w.text ?? ""))
        case "TurnFinished":
            router.observe(.turnFinished(session: session, stopReason: w.stopReason ?? ""))
            catchUp(.turnFinished)
        case "Error":
            router.observe(.error(session: session, message: w.message ?? ""))
        case "ToolCallStarted":
            router.observe(.activity(session: session))
            guard let n = w.name else { return }
            card.state.tools.append(n)
            card.state.status = "working · \(n)"
            // C2 (1fa78c3b): `purpose` is a top-level key of the tool's arguments on every tool; absent → the tool name.
            let args = w.args ?? w.arguments
            let purpose = args?["purpose"]?.string ?? args?["reason"]?.string
            if card.state.command?.observe(.toolCallStarted(id: w.callId ?? n, tool: n, purpose: purpose)) == true { capReached(session) }
            return
        case "ToolCallFinished":
            router.observe(.activity(session: session))
            card.state.command?.observe(.toolCallFinished(id: w.callId ?? "", ok: !(w.isError ?? false)))
            return
        default:
            router.observe(.activity(session: session))
            if let n = w.name { card.state.status = "working · \(n)" }
            return
        }
        renderReply()
    }

    /// The card follows the reply; a finished reply decides the command card, drops the focus claim and may be spoken.
    private func renderReply() {
        guard let reply = router.reply else { return }
        if reply.finished { card.state.command?.finish(text: reply.text, failure: reply.failure) }
        card.state.text = reply.text
        if let c = card.state.command, c.finished {
            card.state.status = c.verdict.map { v -> String in
                switch v { case .ok: return "done"; case .failed: return "failed"; case .replied: return "replied" }
            } ?? "stopped"
        } else {
            card.state.status = reply.finished ? (reply.failureLine ?? "replied") : "working…"
        }
        let failed = reply.failure != nil || card.state.command?.verdict.map { if case .failed = $0 { return true } else { return false } } == true
            || (card.state.command?.frozen?.hasPrefix("✗") ?? false)
        card.state.phase = failed ? .failed : (reply.finished || !reply.text.isEmpty ? .answer : .thinking)
        guard reply.finished else { return }
        PointLog.write("point finished: stop \(reply.stopReason ?? "none"), \(reply.failure.map { "failed — \($0)" } ?? "ok"), \(reply.text.count) chars")
        store.claimFocus(.point, [])
        card.state.turnFinished = true
        if SpokenReplies.shouldSpeak(enabled: VoiceKeySettings.speak, reply: reply) { speaker.speak(reply.text, on: reply.session, from: .voiceKey) } else { armDoneIfReady() }
    }

    /// 189 D2: read the desk's transcript for the reply being collected; the router decides whether it finishes. A probe
    /// never sent anything: its reply finishes on what streamed (the "read failed" path).
    private func catchUp(_ trigger: CatchUpTrigger) {
        guard let r = router.reply, !r.finished else { return }
        Task {
            let loaded = AskFlags.automated ? nil : await store.loadTranscript(r.session)
            guard router.caughtUp(id: r.id, loaded: loaded, trigger: trigger, now: Date()), let reply = router.reply else { return }
            PointLog.write("point catch-up (\(trigger.rawValue)): \(reply.text.count) chars, running \(loaded.map { "\($0.running)" } ?? "unknown")")
            armWatchdog()
            renderReply()
        }
    }

    /// 189 D2: 15 s without a desk event while collecting → read the transcript; re-armed after every event and catch-up.
    private func armWatchdog() {
        watchdog.arm(at: router.reply?.watchdogDeadline) { [weak self] in
            guard let self else { return }
            if self.router.watchdogFired(now: Date()) {
                PointLog.write("point watchdog: no desk event for \(Int(AskReply.watchdog)) s")
                self.catchUp(.watchdog)
            }
            self.armWatchdog()
        }
    }

    /// §10.4: the 12th `ToolCallStarted` → `cancel(desk, root)`; the card already froze with "stopped after 12 steps".
    private func capReached(_ session: String) {
        PointLog.write("command cap reached: cancel(\(session), root)")
        card.state.status = "stopped"
        card.state.phase = .failed
        Task { try? await store.client.cancel(session: session, scope: .root) }
    }

    /// ⎋ on the card: during `thinking` it stops the turn and FREEZES the card (§12.4 — nothing vanishes); otherwise dismiss.
    private func cardEscape() {
        if card.state.phase == .thinking, let s = card.state.session, !card.state.turnFinished {
            PointLog.write("card esc while thinking: cancel(\(s), root)")
            Task { try? await store.client.cancel(session: s, scope: .root) }
            card.state.status = "stopped"
            card.state.phase = .failed
            card.state.turnFinished = true
            return
        }
        switch state {
        case .replying: send(.esc)
        default: card.hide(returnKey: true); doneTimer?.invalidate(); doneTimer = nil; stopSpeaking()
        }
    }

    // MARK: approvals (§10.6)

    private func approvalEvent(_ w: Wire, data: Data) {
        guard card.isVisible, card.state.kind == .command, let s = card.state.session, w.session == s || w.type == "ApprovalResolved" else { return }
        switch w.type {
        case "ApprovalRequested":
            guard let a = try? TheuthJSON.decoder.decode(ApprovalRequested.self, from: data), card.state.approval?.outcome != nil || card.state.approval == nil else { return }
            card.state.approval = .init(id: a.id, tool: a.tool, target: Self.target(of: a), risk: a.risk.rawValue)
            card.state.status = "needs your ok"
            PointLog.write("approval \(a.id) in the card: \(a.tool) \(a.risk.rawValue)")
            beginApprovalListen()
        case "ApprovalResolved":
            guard var a = card.state.approval, a.id == w.id, a.outcome == nil else { return }
            a.outcome = w.timedOut == true ? .timedOut : (w.decision == "allow" ? .allowed : .denied)
            a.listening = false
            card.state.approval = a
            card.state.status = "working…"
            endApprovalListen()
        default: break
        }
    }

    /// The exact target of the call (§10.6 "show the exact target"): the shell command, the script's first line, the
    /// shortcut's name, else the `purpose`, else the tool name.
    static func target(of a: ApprovalRequested) -> String {
        let args = a.args
        // A loop, not a `??` chain: seven optional coalesces in one expression time out the type checker.
        let keys: [String] = ["command", "cmd", "script", "name", "purpose", "path"]
        var s: String = a.tool
        for key in keys {
            if let v = args[key]?.string { s = v; break }
        }
        let first: Substring? = s.split(whereSeparator: { (c: Character) -> Bool in c.isNewline }).first
        let line: String = first.map { String($0) } ?? s
        return line.count > 80 ? String(line.prefix(79)) + "…" : line
    }

    /// Open a short dictation (`Surface.companion`), read the first final utterance against the grammar; `listenSecs` closes it.
    private func beginApprovalListen() {
        guard !AskFlags.automated, !dictation.isActive, isLive == false else { return }
        card.state.approval?.listening = true
        beginDictation(opts: DictationOpts(surface: .companion, destination: .agentPrompt))
        approvalListen?.invalidate()
        approvalListen = Timer.scheduledTimer(withTimeInterval: TimeInterval(SpokenApproval.listenSecs), repeats: false) { [weak self] _ in
            Task { @MainActor in self?.endApprovalListen() }
        }
    }

    private func spokenApproval(_ utterance: String) {
        if let d = SpokenApproval.decide(utterance) {
            PointLog.write("spoken approval “\(utterance)” → \(d)")
            resolveApproval(d, spoken: true)
        } else {
            PointLog.write("spoken approval “\(utterance)” → no match")
            card.state.approval?.missed = true
        }
    }

    private func endApprovalListen() {
        guard approvalListen != nil else { return }
        approvalListen?.invalidate(); approvalListen = nil
        card.state.approval?.listening = false
        recorder.cancel()
        if let id = dictation.id { enqueue { [store] in try await store.client.dictationStop(id: id) } }
        dictation = AskDictation()
        pendingChunks = []; stopWanted = false
    }

    /// ⏎/⎋ or the grammar → `resolve_approval`; the row shows ✓/✗ at once (the engine's `ApprovalResolved` confirms it).
    private func resolveApproval(_ d: Decision, spoken: Bool) {
        guard var a = card.state.approval, a.outcome == nil else { return }
        endApprovalListen()
        a.outcome = d == .allow ? .allowed : .denied
        card.state.approval = a
        card.state.status = "working…"
        PointLog.write("resolve_approval(\(a.id), \(d)) \(spoken ? "spoken" : "key")")
        guard !AskFlags.automated else { return }
        Task { [store] in
            do { _ = try await store.client.resolveApproval(id: a.id, decision: d) } catch { PointLog.write("resolve_approval failed: \(error)") }
        }
    }

    // MARK: the one card for the Ask bar too (§12.1)

    /// `Store.showAnswerCard`: the Ask bar's state replaces ours; the viewer's expansion survives a streaming update.
    func showExternal(_ s: AnswerCardState) {
        var next = s
        if card.isVisible, card.state.generation == s.generation { next.expanded = card.state.expanded; next.current = card.state.current }
        card.state = next
        if !next.visible { card.hide(returnKey: true); return }
        if !card.isVisible { card.show(near: NSEvent.mouseLocation, key: !AskFlags.automated) }
        armDoneIfReady()
    }

    /// The shared speaker's utterance is ours → the glyph; ours ended (played out, stopped, or the ask bar took over) → `doneMs`.
    private func speakingChanged(_ ours: Bool) {
        card.state.speaking = ours
        defer { wasSpeaking = ours }
        if wasSpeaking, !ours { armDoneIfReady() }
    }

    /// ⎋ / conceal ends the voice key's own utterance; the Ask bar's keeps playing.
    private func stopSpeaking() {
        if speaker.surface == .voiceKey { speaker.stop() }
    }

    /// §12.4: `doneMs` only when the card says so (finished ∧ not speaking ∧ collapsed ∧ no open approval); the hide is
    /// generation-checked, so a newer gesture's card never goes with an old timer.
    private func armDoneIfReady() {
        doneTimer?.invalidate(); doneTimer = nil
        guard card.state.armsDone else { return }
        let g = card.state.generation
        doneTimer = Timer.scheduledTimer(withTimeInterval: TimeInterval(AnswerCardState.doneMs) / 1000, repeats: false) { [weak self] _ in
            Task { @MainActor in
                guard let self, self.card.state.armsDone, self.card.state.hide(generation: g) else { return }
                switch self.state {
                case .replying: self.send(.panelDone)
                default: self.card.hide(returnKey: true)
                }
            }
        }
    }
}

/// `voiceKey.*` of 96 (`voice-key.ts:37-47`) in `Settings.defaults` (D19); the Settings › Voice key tab binds the same keys.
enum VoiceKeySettings {
    static let enabledKey = "voiceKeyEnabled", includeScreenKey = "voiceKeyIncludeScreen", accessibilityKey = "voiceKeyAccessibility",
               annotateMouseKey = "voiceKeyAnnotateMouse"
    static var enabled: Bool { flag(enabledKey, true) }
    static var includeScreen: Bool { flag(includeScreenKey, true) }
    /// 96's `accessibility: 'auto' | 'off'` — `true` = auto (the engine's grant decides).
    static var accessibility: Bool { flag(accessibilityKey, true) }
    /// 96's `annotate: 'always' | 'mouse'`.
    static var annotateMouse: Bool { flag(annotateMouseKey, false) }
    /// 149s S1: off unless the user turned it on (was on as 96 shipped); the same key the Settings toggles bind.
    static var speak: Bool { SpokenReplies.voiceKey(Settings.defaults) }

    static func flag(_ key: String, _ def: Bool) -> Bool {
        Settings.defaults.object(forKey: key) == nil ? def : Settings.defaults.bool(forKey: key)
    }
}

/// 153 §10.1: Settings › Command's own switch — the screen is NOT sent with a command by default (the words name the target).
enum CommandSettings {
    static let includeScreenKey = "commandIncludeScreen"
    static var includeScreen: Bool { VoiceKeySettings.flag(includeScreenKey, false) }
}

extension PointController {
    /// The accent colour as drawn (`OverlayView.show` uses `controlAccentColor.cgColor`, which resolves under the app's
    /// effective appearance — the dark-mode accent is a lighter blue than the light-mode one), as sRGB components.
    static var accentSRGB: NSColor {
        var c = NSColor.controlAccentColor
        NSApp.effectiveAppearance.performAsCurrentDrawingAppearance {
            // nil only for pattern colours, never for a system colour (the fallback must be RGB: `redComponent` throws on gray).
            c = c.usingColorSpace(.sRGB) ?? NSColor(srgbRed: 0, green: 0, blue: 0, alpha: 1)
        }
        return c
    }

    /// The trail colour the engine stamps on its capture (`screen_context.color`) as `#rrggbb`, resolved at request time.
    static var accentHex: String {
        let c = accentSRGB
        return OverlayModel.hex(red: c.redComponent, green: c.greenComponent, blue: c.blueComponent)
    }
}

/// stderr plus `/tmp/149-W2-probe.log` during `--probe-point` (and `/tmp/153-c1/probe.log` during the 153 probes).
enum PointLog {
    static let file = URL(fileURLWithPath: "/tmp/149-W2-probe.log")
    static let file153 = URL(fileURLWithPath: "/tmp/153-c1/probe.log")

    static func write(_ s: String) {
        Log.write(s)
        let target: URL
        if Flags.probePoint != nil { target = file } else if Flags.probeCommand != nil || Flags.probeCard != nil { target = file153 } else { return }
        let line = "[companion \(Date()) pid \(getpid())] \(s)\n"
        if let h = try? FileHandle(forWritingTo: target) { h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close() }
        else { try? Data(line.utf8).write(to: target) }
    }
}
