import AppKit
import AVFoundation
import Combine
import CompanionCore
import KeyboardShortcuts
import SwiftUI

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
    /// The press that opened a surface must not close it on release.
    private var openedByPress = false

    init(store: Store, attention: PanelController) {
        self.store = store
        self.attention = attention
        let (panel, container) = GlassPanel.make(width: Self.width, height: 120)
        self.panel = panel
        hosting = NSHostingView(rootView: AskBarView(store: store, state: state))
        hosting.frame = container.bounds
        hosting.autoresizingMask = [.width, .height]
        container.addSubview(hosting)
        super.init()

        state.onSubmit = { [weak self] text in self?.submit(text) }
        state.onTyped = { [weak self] text in self?.typedDuringVoiceHold(text) }
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

    /// Only ⎋ and ⌘O are ours; the field takes the rest (⏎ through `onSubmit`).
    private func handle(_ e: NSEvent) -> Bool {
        let key = KeyPress(chars: e.charactersIgnoringModifiers ?? "", keyCode: e.keyCode, command: e.modifierFlags.contains(.command))
        switch KeyGrammar.intent(for: key) {
        case .escape:
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

    /// Typing during a voice hold keeps editing instead of sending (§4.3).
    private func typedDuringVoiceHold(_ text: String) {
        guard let v = state.voiceText, text != v else { return }
        state.voiceText = nil
        if store.undo.held != nil { store.perform(.escape) }
    }

    // MARK: the hotkey (key-down + key-up from KeyboardShortcuts 1.10.0; W1's toggle lives in `hotkeyUp`)

    func hotkeyDown() {
        guard !AskFlags.automated else { return }
        openedByPress = false
        if isOpen { return voiceEvent(.keyDown(Date())) }
        if attention.isOpen { return } // key-up toggles it closed, as W1 did
        openedByPress = true
        if store.rail.needsYouCount > 0 {
            attention.open(anchor: .mouse)
        } else {
            open()
            voiceEvent(.keyDown(Date())) // a hold from cold dictates straight into the fresh bar
        }
    }

    func hotkeyUp() {
        guard !AskFlags.automated else { return }
        if voice.isBusy { return voiceEvent(.keyUp(Date())) }
        if openedByPress { openedByPress = false; return }
        if attention.isOpen { attention.close(returnKey: true) } else if isOpen { close(returnKey: true) }
    }

    // MARK: voice (D7): hold → AVAudioEngine → release → put_attachment → transcribe → field → the hold

    private func voiceEvent(_ e: AskVoiceMachine.Event) {
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
                if openedByPress { openedByPress = false } else { close(returnKey: true) }
            case .startRecording:
                state.recording = true
                state.status = "listening…"
                Task { [weak self] in
                    guard let self else { return }
                    if let why = await self.recorder.start() { self.voiceEvent(.failed(why)) }
                }
            case .stopAndTranscribe:
                state.recording = false
                state.status = "transcribing…"
                Task { [weak self] in
                    guard let self else { return }
                    do {
                        let wav = self.recorder.stop()
                        guard !wav.isEmpty else { return self.voiceEvent(.failed("Nothing recorded")) }
                        let path = try await self.store.client.putAttachment(wav, ext: "wav")
                        let t = try await self.store.client.transcribe(path: path)
                        AskLog.write("transcribe \(t.durationMs) ms → \(t.text.count) chars")
                        self.voiceEvent(.transcript(t.text))
                    } catch {
                        self.voiceEvent(.failed("\(error)"))
                    }
                }
            case .setText(let text):
                state.status = nil
                state.voiceText = text
                state.text = text
                store.submitAsk(text) // the same 3 s hold; typing or ⎋ keeps editing
            case .fail(let why):
                state.recording = false
                recorder.cancel()
                state.status = why
                AskLog.write("voice: \(why)")
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
        let size = NSSize(width: Self.width, height: max(72, hosting.fittingSize.height))
        let m = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { $0.frame.contains(m) } ?? NSScreen.main!
        let v = screen.visibleFrame
        var p = NSPoint(x: v.midX - size.width / 2, y: v.maxY - v.height * 0.2 - size.height)
        p.x = min(max(v.minX + 8, p.x), v.maxX - size.width - 8)
        p.y = min(max(v.minY + 8, p.y), v.maxY - size.height - 8)
        panel.setFrame(NSRect(origin: p, size: size), display: true)
        hosting.frame = panel.contentView?.bounds ?? .zero
    }
}

/// The Attention panel's glass, for a second panel (W1's `PanelController.init` builds the same by hand).
enum GlassPanel {
    @MainActor
    static func make(width: CGFloat, height: CGFloat) -> (KeyPanel, NSView) {
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
        container.layer?.cornerRadius = 12
        container.layer?.cornerCurve = .continuous
        container.layer?.masksToBounds = true
        container.layer?.borderWidth = 0.5
        container.layer?.borderColor = Palette.border.cgColor

        let backdrop: NSView
        if #available(macOS 26, *) {
            let glass = NSGlassEffectView()
            glass.cornerRadius = 12
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
