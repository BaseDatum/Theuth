import AppKit
import AVFoundation
import Combine
import CompanionCore
import KeyboardShortcuts
import SwiftUI
import TheuthKit

/// The Ask bar (D7, §4.3): a second non-activating, key-when-summoned glass panel, 480 pt, top-centre of the
/// mouse's screen. One field, the hints row, the receipt row. Everything typed goes to theuth's desk (R4).
@MainActor
final class AskBarController: NSObject {
    static let width: CGFloat = 480

    let panel: KeyPanel
    private let store: Store
    private let attention: PanelController
    private let state = AskBarState()
    private let hosting: NSHostingView<AskBarView>
    private var subs = Set<AnyCancellable>()
    private var keyMonitor: Any?
    private var previousApp: NSRunningApplication?
    private(set) var isOpen = false

    // hold-to-dictate
    private var voice = AskVoiceMachine()
    private var voiceTimer: Timer?
    private let recorder = VoiceRecorder()
    /// §8.5: the recorder is open (at key-down, or with `.startRecording`); chunks before the id buffer in `pendingChunks`.
    private var micOpen = false
    /// 153 §6.4: the dictionary suggestion card under the bar (`AskBarInstall` creates it).
    var suggestionCard: DictionarySuggestionCard?

    init(store: Store, attention: PanelController) {
        self.store = store
        self.attention = attention
        let (panel, container) = GlassPanel.make(width: Self.width, height: 120, radius: Type.cardRadius) // 149 W1 (§3.5)
        self.panel = panel
        hosting = NSHostingView(rootView: AskBarView(store: store, state: state, speaker: store.speaker))
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        super.init()

        state.onTyped = { [weak self] text in self?.typedDuringVoiceHold(text) }
        recorder.onChunk = { [weak self] c in self?.feed(c) }
        store.onDictation = { [weak self] d, take in self?.dictationChanged(d, takeText: take) }
        NotificationCenter.default.publisher(for: NSWindow.didResignKeyNotification, object: panel)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.close(returnKey: false) } }
            .store(in: &subs)
        store.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.relayout() } }
            .store(in: &subs)
        state.objectWillChange.receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.relayout() } }
            .store(in: &subs)
        store.onAskBar = { [weak self] in
            guard let self else { return }
            self.attention.close(returnKey: false)
            self.open()
        }
    }

    // MARK: open / close

    /// Summon: remember the frontmost app, place, take key. Never activates this app.
    func open() {
        if attention.isOpen { attention.close(returnKey: false) }
        previousApp = previousApp ?? NSWorkspace.shared.frontmostApplication
        isOpen = true
        place()
        panel.ignoresMouseEvents = false
        panel.makeKeyAndOrderFront(nil)
        state.focusField()
        prewarm()
        if keyMonitor == nil {
            keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
                MainActor.assumeIsolated {
                    guard let self, self.panel.isKeyWindow else { return e }
                    return self.handle(e) ? nil : e
                }
            }
        }
    }

    /// Probe mode: on screen, not key, mouse-transparent, so nothing can lose focus.
    func showWithoutKey(text: String) {
        state.text = text
        isOpen = true
        place()
        panel.ignoresMouseEvents = true
        panel.orderFrontRegardless()
    }

    func close(returnKey: Bool) {
        guard isOpen else { return }
        isOpen = false
        if let m = keyMonitor { NSEvent.removeMonitor(m); keyMonitor = nil }
        if voice.isBusy { apply(voice.handle(.failed("cancelled"))) }
        panel.orderOut(nil)
        if returnKey, let app = previousApp, app != .current { app.activate() }
        previousApp = nil
    }

    func toggle() { isOpen ? close(returnKey: true) : open() }

    // MARK: keys

    /// The table's Ask-bar scope: ⏎ sends, ⎋, ⌘O, ⌘/; the field takes the rest.
    private func handle(_ e: NSEvent) -> Bool {
        switch KeyGrammar.intent(for: LegendController.press(e), scope: .askBar, table: SettingsModel.shared.keys) {
        case .send:
            submit(state.text)
            return true
        case .legend:
            store.onLegend?()
            return true
        case .escape:
            store.speaker.stop() // S3: ⎋ silences the spoken reply, whatever else it does
            if store.undo.held != nil { store.perform(.escape); return true } // undo: the text is still in the field
            close(returnKey: true)
            return true
        case .openSession:
            if case .reply(_, _, let s?) = store.receipt { store.onOpenURL?(DeepLink.session(s)) }
            else if let s = store.ask.router.deskSession { store.onOpenURL?(DeepLink.session(s)) }
            return true
        default:
            return false
        }
    }

    private func submit(_ text: String) {
        guard !AskFlags.automated else { return AskLog.write("probe: send suppressed") }
        state.voiceText = nil
        store.submitAsk(text)
    }

    /// Typing during a voice hold keeps editing instead of sending (§4.3); typed edits win over late partials. An emptied
    /// field leaves nothing to correct (153 §6.1).
    private func typedDuringVoiceHold(_ text: String) {
        guard let v = state.voiceText, text != v else { return }
        state.voiceText = nil
        _ = store.ask.dictation.handle(.typed)
        if text.isEmpty { _ = store.ask.dictation.handle(.settled) }
        if store.undo.held != nil { store.perform(.escape) }
    }

    // MARK: the panel key (key-down + key-up from KeyboardShortcuts 1.10.0)

    /// 149s K1: every press goes through the hold machine. A tap (`.tap`) toggles the Attention panel — empty state
    /// included, never the Ask bar; a hold (`.startRecording`) talks: the Ask bar opens if it is not up and dictates.
    /// 153 §8.5: with "Open the mic on key-down" the recorder opens at the press and a tap discards it.
    func hotkeyDown() {
        guard !AskFlags.automated else { return }
        voice.micOnKeyDown = ModeKeySettings.micOnKeyDown(Settings.defaults)
        voiceEvent(.keyDown(Date()))
    }

    func hotkeyUp() {
        guard !AskFlags.automated else { return }
        voiceEvent(.keyUp(Date()))
    }

    private func perform(_ a: SurfaceKey.Action) {
        switch a {
        case .togglePanel: attention.toggle(anchor: .mouse) // taking key closes an open Ask bar (`didResignKey`)
        case .toggleAskBar: toggle()
        case .talk: if !isOpen { open() }
        }
    }

    // MARK: voice (D7, W6 live): hold → AVAudioEngine → dictation_audio every 100 ms → release → dictation_stop
    // → DictationFinished → the text stays in the field → the hold

    /// The hotkey's events into the machine; the probe (`--synthetic-dictation`) drives the same entry.
    func voiceEvent(_ e: AskVoiceMachine.Event) {
        apply(voice.handle(e))
        if voice.isBusy, voiceTimer == nil {
            voiceTimer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.voiceTick() }
            }
        } else if !voice.isBusy {
            voiceTimer?.invalidate(); voiceTimer = nil
        }
    }

    private func voiceTick() {
        apply(voice.handle(.tick(Date())))
        state.recording = voice.isRecording
        if !voice.isBusy { voiceTimer?.invalidate(); voiceTimer = nil }
    }

    private func apply(_ effects: [AskVoiceMachine.Effect]) {
        for fx in effects {
            switch fx {
            case .tap:
                perform(SurfaceKey.panel.action(held: false))
            case .openMic:
                startMic()
            case .discardMic:
                micOpen = false
                recorder.cancel()
                pendingChunks = []
                AskLog.write("voice: tap — mic discarded, no dictation")
            case .startRecording:
                perform(SurfaceKey.panel.action(held: true))
                state.recording = true
                state.status = "listening…"
                beginDictation()
                if !micOpen { startMic() }
            case .stopDictation:
                state.recording = false
                state.status = "finishing…"
                micOpen = false
                recorder.stop() // flushes the tail chunk through `onChunk`
                endDictation()
            case .setText(let text):
                landed(text)
                if AskFlags.automated { AskLog.write("probe: send suppressed") } else { store.submitAsk(text) } // the same 3 s hold; typing or ⎋ keeps editing
            case .sendNow(let text):
                landed(text)
                if AskFlags.automated { AskLog.write("probe: send suppressed (send it)") } else { store.sendAskNow(text) } // §7.4: no hold
            case .fail(let why):
                state.recording = false
                state.live = false
                micOpen = false
                recorder.cancel()
                // 149g: every exit stops what it started and ends the hold — a `dictation_start` still in flight lands into a
                // reset session and stops its own id (`beginDictation`), instead of being adopted by nobody.
                if let id = store.ask.dictation.id { enqueue { [store] in try await store.client.dictationStop(id: id) } }
                store.ask.dictation = AskDictation()
                pendingChunks = []; stopWanted = false
                state.status = why
                AskLog.write("voice: \(why)")
            }
        }
    }

    /// The dictated text is in the field; the engine ended the dictation (or we did), so the microphone closes here.
    private func landed(_ text: String) {
        state.recording = false
        state.status = nil
        state.live = false
        state.voiceText = text
        state.text = text
        micOpen = false
        recorder.cancel()
    }

    /// A mic refusal landing after this hold ended (Esc, close) must not fail the next one. Probes never open the mic.
    private func startMic() {
        micOpen = true
        guard !AskFlags.automated else { return }
        let g = hold
        Task { [weak self] in
            guard let self, let why = await self.recorder.start(), g == self.hold else { return }
            self.voiceEvent(.failed(why))
        }
    }

    // MARK: the engine's dictation (design 54 §8): one id per hold; audio in order; the reducer in `store.ask`

    private var sendTask: Task<Void, Never>?
    private var prewarmed = false
    private var pendingChunks: [[Int16]] = []
    /// The release came before `dictation_start` replied.
    private var stopWanted = false
    /// 149g: bumped by every `beginDictation`; the start's reply and refusal belong to the hold that sent them (`awaitsStart`).
    private var hold = 0

    /// Calls run strictly in order (audio chunks, then the stop).
    private func enqueue(_ op: @escaping () async throws -> Void) {
        let prev = sendTask
        sendTask = Task {
            await prev?.value
            do { try await op() } catch { AskLog.write("dictation call failed: \(error)") }
        }
    }

    /// `dictation_prewarm` once, when the Ask bar first opens (not at launch).
    private func prewarm() {
        guard !prewarmed else { return }
        prewarmed = true
        Task { [store] in
            do { try await store.client.dictationPrewarm(); AskLog.write("dictation_prewarm ok") }
            catch { AskLog.write("dictation_prewarm failed: \(error)") }
        }
    }

    /// `dictation_start(16000, {surface: companion, destination: agent_prompt, press_ago_ms})` (153 §14.2); chunks that arrive
    /// before the id are held and flushed in order.
    func beginDictation() {
        _ = store.ask.dictation.handle(.start)
        state.live = true
        state.partial = ""
        stopWanted = false
        hold += 1
        let g = hold
        let pressed = voice.pressedAt
        enqueue { [weak self, store] in
            do {
                if let d = self?.probeStartDelay, d > .zero { try? await Task.sleep(for: d) }
                let ago = pressed.map { UInt32(max(0, Date().timeIntervalSince($0)) * 1000) }
                let id = try await store.client.dictationStart(opts: DictationOpts(surface: .companion, destination: .agentPrompt, pressAgoMs: ago))
                guard let self else { return }
                guard store.ask.dictation.awaitsStart(from: g, current: self.hold) else {
                    AskLog.write("dictation_start → \(id) landed after the hold ended: dictation_stop")
                    return try await store.client.dictationStop(id: id)
                }
                AskLog.write("dictation_start → \(id)")
                _ = store.ask.dictation.handle(.started(id: id))
                for c in self.pendingChunks { self.send(c, id: id) }
                self.pendingChunks = []
                if self.stopWanted { self.stopWanted = false; self.endDictation() } // released before the start came back
            } catch {
                guard let self else { return }
                AskLog.write("dictation_start failed: \(error)")
                guard store.ask.dictation.awaitsStart(from: g, current: self.hold) else { return } // a stale hold's refusal
                _ = store.ask.dictation.handle(.startFailed("\(error)"))
                let why = store.ask.dictation.error ?? "\(error)"
                self.voiceEvent(.failed(why))
                self.probeFailed?(why)
            }
        }
    }

    /// One ~100 ms chunk from the microphone (or the probe's synthesizer). §8.5: chunks before the id — the start in flight, or
    /// the mic opened at key-down and the press not yet a hold — wait in order; a tap drops them (`.discardMic`).
    func feed(_ chunk: [Int16]) {
        if let id = store.ask.dictation.id { send(chunk, id: id) } else if store.ask.dictation.phase == .starting || micOpen { pendingChunks.append(chunk) }
    }

    private func send(_ chunk: [Int16], id: String) {
        enqueue { [store] in try await store.client.dictationAudio(id: id, pcm16: chunk) }
    }

    /// `dictation_stop` after every queued chunk; the text is final once `DictationFinished` arrives.
    func endDictation() {
        guard let id = store.ask.dictation.id else { stopWanted = store.ask.dictation.phase == .starting; return }
        _ = store.ask.dictation.handle(.stop)
        enqueue { [store] in try await store.client.dictationStop(id: id); AskLog.write("dictation_stop(\(id)) ok") }
        // ponytail: a 5 s cap on "finishing…" if the engine never sends DictationFinished; the text so far lands.
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) { [weak self] in
            guard let self, case .stopping = self.voice.state else { return }
            self.voiceEvent(.dictated(self.store.ask.dictation.liveText))
        }
    }

    /// The Store reduced a `Dictation*` event: show the live text, pulse the mic, finish the machine. 153: a `DictationDevice`
    /// hint and "send it" arming show in the status slot; a finish while still recording (the engine settled "send it", §7.3)
    /// lands like a release.
    private func dictationChanged(_ d: AskDictation, takeText: Bool) {
        if takeText, state.live {
            state.text = d.finalText
            state.partial = d.partialText
        }
        state.level = d.levels.max() ?? 0
        if d.isActive, state.live {
            if let h = d.deviceHint { state.status = h } else if d.send { state.status = "sending…" }
        }
        guard !d.isActive, voice.isBusy, !voice.isPressed else { return } // pressed: no dictation of ours yet (a foreign finish)
        voiceEvent(d.error.map { .failed($0) } ?? .dictated(d.liveText, send: d.send))
    }

    // MARK: --synthetic-dictation (probe): 2 s of a 440 Hz tone through the same path, no microphone, no send

    var probeFailed: ((String) -> Void)?
    /// 149g probe only: `dictation_start` is held this long before it is sent (`PointController.probeStartDelay`'s twin), so an
    /// Esc/close/release that beats the reply is reproducible against a live engine.
    var probeStartDelay: Duration = .zero
    var voiceState: AskVoiceMachine.State { voice.state }
    var probeStatus: String? { state.status }

    func probeDictation(seconds: TimeInterval, done: @escaping () -> Void) {
        prewarm()
        var sent = 0
        let total = Int(seconds * 10)
        beginDictation()
        Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] t in
            Task { @MainActor in
                guard let self else { return t.invalidate() }
                if sent < total {
                    let start = sent * VoiceRecorder.chunk
                    self.feed((0..<VoiceRecorder.chunk).map { i in Int16(sin(2 * .pi * 440 * Double(start + i) / VoiceRecorder.sampleRate) * 0.3 * 32767) })
                    sent += 1
                } else {
                    t.invalidate()
                    self.endDictation()
                    done()
                }
            }
        }
    }

    // MARK: menu

    /// "Ask theuth…" after "Open theuth" (W1's menu; W2 appends its own items).
    func addMenuItem(to menu: NSMenu) {
        let item = NSMenuItem(title: "Ask theuth…", action: #selector(askFromMenu), keyEquivalent: "")
        item.target = self
        menu.insertItem(item, at: min(1, menu.items.count))
    }

    @objc private func askFromMenu() { open() }

    // MARK: placement (top-centre of the mouse's screen, ≈ 20 % down, like the Attention panel's `.mouse`)

    private func relayout() {
        DispatchQueue.main.async { [self] in if isOpen { place() } }
    }

    private func place() {
        hosting.layoutSubtreeIfNeeded()
        let frame = Self.frame(for: NSSize(width: Self.width, height: max(72, hosting.fittingSize.height)))
        panel.setFrame(frame, display: true)
        hosting.frame = panel.contentView?.bounds ?? .zero
    }

    /// Where a `size` panel sits: top-centre of the mouse's screen, ≈ 20 % down, clamped 8 pt inside the visible frame.
    static func frame(for size: NSSize) -> NSRect {
        let m = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { $0.frame.contains(m) } ?? NSScreen.main!
        let v = screen.visibleFrame
        var p = NSPoint(x: v.midX - size.width / 2, y: v.maxY - v.height * 0.2 - size.height)
        p.x = min(max(v.minX + 8, p.x), v.maxX - size.width - 8)
        p.y = min(max(v.minY + 8, p.y), v.maxY - size.height - 8)
        return NSRect(origin: p, size: size)
    }

    /// 153 §6.4: the suggestion card hangs under the Ask bar — its frame while open, else where it would open.
    var anchorFrame: NSRect { isOpen ? panel.frame : Self.frame(for: NSSize(width: Self.width, height: 72)) }
}

/// The Attention panel's glass, for a second panel (W1's `PanelController.init` builds the same by hand).
enum GlassPanel {
    /// 149 W1: `radius` per surface (`Type.cardRadius` default); appearance follows the system.
    @MainActor
    static func make(width: CGFloat, height: CGFloat, radius: CGFloat = 12) -> (KeyPanel, NSView) {
        let panel = KeyPanel(contentRect: NSRect(x: 0, y: 0, width: width, height: height),
                             styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient]
        panel.sharingType = Flags.shareable ? .readOnly : .none
        panel.becomesKeyOnlyIfNeeded = false
        panel.hidesOnDeactivate = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isReleasedWhenClosed = false
        panel.animationBehavior = .utilityWindow

        let container = NSView(frame: NSRect(x: 0, y: 0, width: width, height: height))
        container.wantsLayer = true
        container.layer?.cornerRadius = radius
        container.layer?.cornerCurve = .continuous
        container.layer?.masksToBounds = true
        container.layer?.borderWidth = 0.5
        container.layer?.borderColor = NSColor.separatorColor.cgColor

        let backdrop: NSView
        if #available(macOS 26, *) {
            let glass = NSGlassEffectView()
            glass.cornerRadius = radius
            backdrop = glass
        } else {
            let effect = NSVisualEffectView()
            effect.material = .hudWindow
            effect.blendingMode = .behindWindow
            effect.state = .active
            backdrop = effect
        }
        backdrop.frame = container.bounds
        backdrop.autoresizingMask = [.width, .height]
        container.addSubview(backdrop)
        panel.contentView = container
        return (panel, container)
    }
}
