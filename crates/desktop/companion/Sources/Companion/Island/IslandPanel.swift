import AppKit
import Combine
import CompanionCore
import KeyboardShortcuts
import SwiftUI
import TheuthKit

extension KeyboardShortcuts.Name {
    /// ⌥` / ⌥⇧E (`KeyAction.meetingPtt` / `.meetingEye`), enabled only while a meeting is live (§8).
    static let meetingPtt = Self("meetingPtt", default: .init(.backtick, modifiers: [.option]))
    static let meetingEye = Self("meetingEye", default: .init(.e, modifiers: [.option, .shift]))
}

extension AppDelegate {
    /// The one island; `install()` is the line AppDelegate calls at the reserved point.
    var island: IslandController { IslandController.shared(store: store) }
}

/// 149 W3 — the native island (§3.6, D9–D11, §5.4 C), 149i rulings D1–D4: visible iff the meeting is live or "Always
/// show" (`IslandModel.visible`, the Tauri original's rule, no linger); live → the strip + brief pane at once, no hover
/// (a click on the strip/pill collapses ↔ opens, per meeting); the window placed on the notched screen (else the primary)
/// at the notch or the fallback pill; a `performDrag` grip with snap-back within 40 pt and per-display-UUID frame memory;
/// the meeting flows over the Store's slice.
@MainActor
final class IslandController {
    private static var instance: IslandController?
    static func shared(store: Store) -> IslandController {
        if let i = instance { return i }
        let i = IslandController(store: store); instance = i; return i
    }

    let window: IslandWindow
    let ui = IslandUI()
    private(set) var model = IslandModel()
    let store: Store
    let hosting: IslandHostingView<IslandRoot>
    private var subs = Set<AnyCancellable>()
    private var tick: Timer?
    private(set) var dragPoll: Timer?
    private var stopTimer: Timer?
    private var oracleTimer: Timer?
    private var keyMonitor: Any?
    private var previousApp: NSRunningApplication?
    /// The window's origin when the grip was pressed: unchanged at mouse-up = a click, which collapses ↔ opens (D1).
    private var dragStart: NSPoint?
    private var cursorSent: String?
    private var cursorEverSent = false
    /// The slice as last seen, for growth detection (the publisher's timing is not relied on).
    private var seen = MeetingSlice.empty
    private var hidingWork: DispatchWorkItem?
    private(set) var shown = false
    /// Probe switches: the fallback pill forced; a pretend notch on a Mac without one.
    var forceNoNotch = false
    var fakeNotch = false

    private init(store: Store) {
        self.store = store
        window = IslandWindow()
        hosting = IslandHostingView(rootView: IslandRoot(ui: ui, store: store))
        hosting.frame = window.contentView!.bounds
        hosting.autoresizingMask = [.width, .height]
        window.contentView!.addSubview(hosting)
        model.frames = IslandSettings.frames
        model.alwaysShow = IslandSettings.alwaysShow
        ui.eyeHidden = IslandSettings.eyeHidden
    }

    /// Wire everything; called once from `applicationDidFinishLaunching`.
    func install() {
        ui.onGrip = { [weak self] e in e.clickCount == 2 ? self?.snapBack() : self?.beginDrag(e) }
        ui.onCollapseToggle = { [weak self] in self?.toggleCollapsed() }
        // §N: a click anywhere on the drawn island makes the panel key (J/K/↑/↓ then cycle the briefs) without focusing the
        // request box — only a click on the box focuses its field. Never swallows the event.
        window.wantsKeyOnClick = { [weak self] p in self?.drawnAppKit.contains(p) ?? false }
        store.$meeting.receive(on: RunLoop.main).sink { [weak self] s in MainActor.assumeIsolated { self?.meetingChanged(s) } }.store(in: &subs)
        store.$connection.receive(on: RunLoop.main).sink { [weak self] c in
            MainActor.assumeIsolated {
                guard let self, c == .open, !Flags.islandProbe else { return }
                Task { await self.store.meetingHydrate() }
            }
        }.store(in: &subs)
        ui.objectWillChange.receive(on: RunLoop.main).sink { [weak self] _ in MainActor.assumeIsolated { self?.uiChanged() } }.store(in: &subs)
        ui.$eyeHidden.dropFirst().receive(on: RunLoop.main).sink { [weak self] h in MainActor.assumeIsolated { self?.setEye(h) } }.store(in: &subs)
        NotificationCenter.default.publisher(for: NSApplication.didChangeScreenParametersNotification)
            .receive(on: RunLoop.main).sink { [weak self] _ in MainActor.assumeIsolated { self?.screensChanged() } }.store(in: &subs)
        NotificationCenter.default.publisher(for: NSWindow.didResignKeyNotification, object: window)
            .receive(on: RunLoop.main).sink { [weak self] _ in MainActor.assumeIsolated { self?.ui.blurTick += 1 } }.store(in: &subs)
        installHotkeys()
        setEye(ui.eyeHidden)
        if let p = Flags.probeIsland { runIslandProbe(p) } else if Flags.probeEye { runEyeProbe() }
    }

    // MARK: visibility (D2) · layout (D1)

    private func meetingChanged(_ s: MeetingSlice?) {
        let slice = s ?? .empty
        if slice.oracle.last != seen.oracle.last { fadeVerdict() }
        let moved = slice.cursor != seen.cursor || slice.current?.id != seen.current?.id
        seen = slice
        // The focused topic follows the shown brief — one path for the chevrons, J/K/↑/↓ and the stack's own moves (§N).
        if moved, let t = slice.current?.topic, let i = slice.topics.firstIndex(where: { $0.id == t }), ui.focusedTopic != i { ui.focusedTopic = i }
        model.meetingChanged(to: slice.meeting)
        if !Flags.islandProbe { setLiveHotkeys(slice.isLive || slice.state == .starting) }
        refreshVisibility()
        applyLayout()
        mirrorCursor()
    }

    private func refreshVisibility() {
        let want = IslandModel.visible(seen, alwaysShow: model.alwaysShow)
        if want, !shown { show() } else if !want, shown { hide() }
    }

    /// Attached, the layout follows the meeting: live → open unless collapsed, else the pill. Detached stays as it is. Only
    /// while shown: once `visible` turned false the layout is frozen and fades as it was (R1 — Done must never flash the
    /// not-live pill); the pill layout appears only live-and-collapsed or as the idle wordmark with "Always show" on.
    private func applyLayout() {
        guard shown, !ui.isDetached, dragPoll == nil else { return }
        let want = IslandModel.attached(live: IslandModel.live(seen), collapsed: model.collapsed)
        guard ui.state != want else { return }
        ui.state = want
        if want == .closed { ui.menuOpen = false }
    }

    /// A click on the strip's grip or the live pill: collapsed ↔ open for this meeting.
    func toggleCollapsed() {
        model.toggleCollapsed(meeting: seen.meeting)
        applyLayout()
        IslandLog.write("island collapsed=\(model.collapsed) state=\(ui.state)")
    }

    func show() {
        hidingWork?.cancel(); hidingWork = nil
        shown = true
        NSAnimationContext.runAnimationGroup { $0.duration = 0; window.animator().alphaValue = 1 }
        restoreOrDefault()
        applyLayout()
        place()
        window.show()
        startTick()
        IslandLog.write("island shown state=\(ui.state) anchor=\(ui.anchor.map { "\($0.rect) notch=\($0.notch)" } ?? "none") frame=\(window.frame) level=\(window.level.rawValue) sharing=\(window.sharingType.rawValue) number=\(window.windowNumber)")
    }

    /// Done/Failed/Idle (or "Always show" off): a 0.3 s fade, then `orderOut` — nothing stays on screen at alpha 0.
    func hide() {
        shown = false
        ui.menuOpen = false
        NSAnimationContext.runAnimationGroup { $0.duration = 0.3; window.animator().alphaValue = 0 }
        let w = DispatchWorkItem { [weak self] in
            guard let self, !self.shown else { return }
            self.window.orderOut(nil)
            self.window.alphaValue = 1
            self.tick?.invalidate(); self.tick = nil
            IslandLog.write("island hidden")
        }
        hidingWork = w
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3, execute: w)
    }

    /// The clock (`ui.now`) once a second while shown; the oracle chip every 30 s.
    private func startTick() {
        guard tick == nil else { return }
        tick = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.ui.now = Date() }
        }
        oracleTimer?.invalidate()
        oracleTimer = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.pollOracle() }
        }
        pollOracle()
    }

    // MARK: placement (D9: notch geometry from AppKit, frames per display UUID, validated)

    var screen: NSScreen? { IslandGeometry.targetScreen(forceNoNotch: forceNoNotch) }

    /// The saved frame for this display (validated against its visibleFrame) → detached; else the notch default.
    private func restoreOrDefault() {
        guard let s = screen else { return }
        let uuid = IslandGeometry.uuid(of: s)
        let visible = IslandGeometry.relative(s.visibleFrame, to: s)
        if let f = model.frame(for: uuid, visibleFrames: [visible]) {
            ui.state = .detached(frame: IslandGeometry.global(f, from: s))
            IslandLog.write("frame restored for \(uuid): \(f) (screen-relative)")
        } else {
            if let saved = model.frames[uuid] {
                IslandLog.write("saved frame \(saved) for \(uuid) intersects no visibleFrame \(visible) — rejected, notch default used")
                model.frames[uuid] = nil
                IslandSettings.frames = model.frames
            }
            if ui.isDetached { ui.state = .closed }
        }
    }

    /// Attached: the oversized window centred on the anchor, flush with the screen top. Detached: the remembered frame.
    private func place(animate: Bool = false) {
        guard let s = screen else { return }
        let anchor = IslandGeometry.anchor(on: s, forceNoNotch: forceNoNotch, fakeNotch: fakeNotch)
        if ui.anchor != anchor { ui.anchor = anchor }
        let target: NSRect
        switch ui.state {
        case .detached(let f):
            // The content reports its own size (`ui.drawn`); the window converges on it, top-left held.
            let size = ui.drawn.size == .zero ? f.size : ui.drawn.size
            target = NSRect(x: f.minX, y: f.maxY - size.height, width: max(size.width, IslandLayout.detachedWidth), height: size.height)
        default:
            target = NSRect(x: anchor.rect.midX - IslandLayout.windowWidth / 2, y: s.frame.maxY - IslandLayout.windowHeight,
                            width: IslandLayout.windowWidth, height: IslandLayout.windowHeight)
        }
        if window.frame != target { window.setFrame(target, display: true, animate: animate && window.isVisible) }
        hosting.frame = window.contentView?.bounds ?? .zero
    }

    private func screensChanged() {
        guard dragPoll == nil else { return }
        IslandLog.write("screens changed: \(NSScreen.screens.map { "\($0.localizedName) \($0.frame) notch=\(IslandGeometry.notch(of: $0) != nil)" })")
        if shown { restoreOrDefault(); applyLayout(); place() }
    }

    /// SwiftUI's global (top-left) rect → the hosting view's AppKit (bottom-left) rect.
    func flip(_ r: CGRect) -> NSRect {
        NSRect(x: r.minX, y: hosting.bounds.height - r.maxY, width: r.width, height: r.height)
    }
    func flip(_ p: NSPoint) -> CGPoint { CGPoint(x: p.x, y: hosting.bounds.height - p.y) }
    var drawnAppKit: NSRect { flip(ui.drawn) }
    /// The drawn island in global screen coordinates.
    var drawnGlobal: NSRect { drawnAppKit.offsetBy(dx: window.frame.minX, dy: window.frame.minY) }

    private func uiChanged() {
        DispatchQueue.main.async { [self] in
            // `IslandHostingView` is flipped: SwiftUI's global rect is already in its coordinates (no `flip` — that was the
            // click-through bug, 149i). Detached, the window is the content: everything is hittable.
            hosting.drawn = ui.isDetached ? nil : ui.drawn
            if shown, dragPoll == nil { place() }
        }
    }

    // MARK: drag · snap-back · memory (§3.6)

    private func beginDrag(_ e: NSEvent) {
        guard dragPoll == nil else { return }
        previousApp = previousApp ?? NSWorkspace.shared.frontmostApplication
        dragStart = window.frame.origin
        window.performDrag(with: e)
        // `performDrag` may never deliver the mouse-up: poll the button until it is released.
        dragPoll = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] t in
            Task { @MainActor in
                guard let self else { return t.invalidate() }
                if NSEvent.pressedMouseButtons & 1 == 0 { t.invalidate(); self.dragPoll = nil; self.dragEnded() }
            }
        }
    }

    private func dragEnded() {
        guard let s = window.screen ?? screen else { return }
        let anchor = IslandGeometry.anchor(on: s, forceNoNotch: forceNoNotch, fakeNotch: fakeNotch)
        let wasDetached = ui.isDetached
        let drawn = wasDetached ? window.frame : drawnGlobal
        let uuid = IslandGeometry.uuid(of: s)
        let moved = dragStart.map { hypot(window.frame.minX - $0.x, window.frame.minY - $0.y) } ?? 0
        dragStart = nil
        model.dragEnded(frame: IslandGeometry.relative(drawn, to: s), notch: IslandGeometry.relative(anchor.rect, to: s), display: uuid)
        IslandSettings.frames = model.frames
        switch model.state {
        case .closed:
            ui.state = .closed
            if moved < 2, !wasDetached {
                // A press and release on the strip without a drag: the D1 click.
                toggleCollapsed()
            } else {
                IslandLog.write("drag ended at \(drawn): within \(IslandModel.snapBack) pt of \(anchor.rect) — snap back")
                applyLayout()
            }
            place(animate: true)
        case .detached:
            IslandLog.write("drag ended at \(drawn): detached, remembered for \(uuid) as \(model.frames[uuid]!)")
            ui.state = .detached(frame: drawn)
            place()
        case .open: break
        }
    }

    /// Double-click the grip (or Settings › Island › Reset): back to the notch, the saved frame cleared.
    func snapBack(all: Bool = false) {
        if all { model.frames = [:] } else if let s = screen { model.frames[IslandGeometry.uuid(of: s)] = nil }
        IslandSettings.frames = model.frames
        ui.state = .closed
        applyLayout()
        if shown { place(animate: true) }
        IslandLog.write("snap back (all=\(all)) frames=\(model.frames.count)")
    }

    func setAlwaysShow(_ on: Bool) {
        model.alwaysShow = on
        IslandSettings.alwaysShow = on
        refreshVisibility()
    }

    // MARK: eye (best effort, §9; Revision 1 row 9)

    private func setEye(_ hidden: Bool) {
        IslandSettings.eyeHidden = hidden
        let type: NSWindow.SharingType = (hidden && !Flags.shareable) ? .none : .readOnly
        for w in NSApp.windows where w is NSPanel { w.sharingType = type }
        IslandLog.write("eye hidden=\(hidden) → sharingType=\(type.rawValue) on \(NSApp.windows.filter { $0 is NSPanel }.count) panels")
    }

    // MARK: keys — island scope (§8) + the global PTT/eye rows

    private func installHotkeys() {
        KeyboardShortcuts.onKeyDown(for: .meetingPtt) { [weak self] in
            guard let self, !AskFlags.automated else { return }
            self.store.meetingPtt(true)
        }
        KeyboardShortcuts.onKeyUp(for: .meetingPtt) { [weak self] in
            guard let self, !AskFlags.automated else { return }
            self.store.meetingPtt(false)
        }
        KeyboardShortcuts.onKeyUp(for: .meetingEye) { [weak self] in
            guard let self, !AskFlags.automated else { return }
            self.ui.eyeHidden.toggle()
        }
        setLiveHotkeys(false)
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            MainActor.assumeIsolated {
                guard let self, self.window.isKeyWindow else { return e }
                return self.handle(e) ? nil : e
            }
        }
    }

    /// PTT/eye are registered only while a meeting is live (§8 anywhere rows).
    private func setLiveHotkeys(_ live: Bool) {
        for n in [KeyboardShortcuts.Name.meetingPtt, .meetingEye] { live ? KeyboardShortcuts.enable(n) : KeyboardShortcuts.disable(n) }
    }

    private func handle(_ e: NSEvent) -> Bool {
        let press = LegendController.press(e)
        guard let action = SettingsModel.shared.keys.action(for: press.chord, in: .island) else { return false }
        // §N: J/K/↑/↓ cycle the briefs (wrapping) — in the box J/K type, the arrows cycle only while it is empty.
        if action == .islandNext || action == .islandPrevious {
            guard IslandModel.cyclesBrief(arrow: !press.chord.isCharKey, boxFocused: ui.boxFocused, textEmpty: ui.requestText.isEmpty) else { return false }
            store.meetingMove(action == .islandNext ? -1 : 1, wrap: true) // next = down the stack (older), as the pane's ↓ chevron
            return true
        }
        // While the box has focus single keys type; only ⏎ and ⎋ act (KeyGrammar's rule).
        if ui.boxFocused, action != .islandSend, action != .islandEscape { return false }
        let s = store.meeting ?? .empty
        switch action {
        case .islandMute:
            s.state == .paused ? store.meetingResume() : store.meetingPause()
        case .islandStop:
            guard ui.stopHoldStarted == nil else { return true }
            ui.stopHoldStarted = Date()
            stopTimer = Timer.scheduledTimer(withTimeInterval: Settings.undoDelay, repeats: false) { [weak self] _ in
                Task { @MainActor in
                    guard let self, self.ui.stopHoldStarted != nil else { return }
                    self.ui.stopHoldStarted = nil
                    self.store.meetingStop()
                }
            }
        case .islandDismiss:
            if s.topics.indices.contains(ui.focusedTopic) { store.meetingDismiss(s.topics[ui.focusedTopic].id) }
        case .islandDeeper:
            if let b = s.current { store.meetingDeeper(b.id) }
        case .islandSend:
            return false // the TextField's onSubmit sends
        case .islandEscape:
            escape()
        default:
            return false
        }
        return true
    }

    /// ⎋: leave the box (the island stays key, §N) → undo a stop hold → close the ⋮ menu and hand key back to the previous app.
    private func escape() {
        if ui.boxFocused { ui.blurTick += 1; return }
        if ui.stopHoldStarted != nil { ui.stopHoldStarted = nil; stopTimer?.invalidate(); return }
        ui.menuOpen = false
        resignKey()
    }

    private func resignKey() {
        if window.isKeyWindow { window.resignKey() }
        if let app = previousApp, app != .current { app.activate() }
        previousApp = nil
    }

    // MARK: the oracle chip · the cursor mirror

    private func pollOracle() {
        guard shown, store.connection == .open, !Flags.islandProbe else { return }
        Task { [weak self] in
            guard let self else { return }
            if let m = try? await self.store.client.decisionMode(prefix: "meeting") { self.ui.oracle = m }
        }
    }

    private func fadeVerdict() {
        ui.verdictFaded = false
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) { [weak self] in self?.ui.verdictFaded = true }
    }

    /// Every cursor move → `meeting_cursor` so the control centre mirrors it (03 §0.4); only while shown.
    private func mirrorCursor() {
        guard shown else { return }
        let id = seen.current?.id
        if cursorEverSent, cursorSent == id { return }
        cursorEverSent = true; cursorSent = id
        store.meetingCursor(id)
    }

    // MARK: probes (§10): --probe-island <closed|open|detached|nonotch> · --probe-eye

    private func runIslandProbe(_ which: String) {
        forceNoNotch = which.hasSuffix("nonotch")
        fakeNotch = !forceNoNotch && NSScreen.screens.allSatisfy { IslandGeometry.notch(of: $0) == nil }
        if fakeNotch { IslandLog.write("probe: no screen has a notch — a 200 × 37 notch is pretended at the top centre") }
        guard let s = screen else { return IslandLog.write("probe: no screen") }
        if which.hasPrefix("clicks") || which.hasPrefix("stop") || which.hasPrefix("open-no-hover") {
            Task { await runInputProbe(which) }
            return
        }
        let uuid = IslandGeometry.uuid(of: s)
        let visible = IslandGeometry.relative(s.visibleFrame, to: s)
        // The validation path: an off-screen frame saved under this display is rejected and logged; then, for the
        // detached state, a good one is saved and restored.
        model.frames[uuid] = NSRect(x: visible.maxX + 500, y: 100, width: 440, height: 300)
        restoreOrDefault()
        if which == "detached" {
            model.frames[uuid] = NSRect(x: 40, y: visible.maxY - 60 - 360, width: 440, height: 360)
            IslandSettings.frames = model.frames
        }
        store.meeting = IslandFixture.slice()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [self] in
            if which == "closed" { toggleCollapsed() } // live → open by default (D1); the pill is the user's collapse
            ui.transcriptOpen = which == "detached"
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.9) { [self] in self.logProbeState(which) }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + Flags.probeSeconds) { [self] in
            if which == "detached" { model.frames[uuid] = nil; IslandSettings.frames = model.frames } // never leave a probe frame behind
            IslandLog.write("probe done — quitting 0 (active=\(NSApp.isActive) key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none"))")
            NSApp.terminate(nil)
        }
    }

    private func runEyeProbe() {
        store.meeting = IslandFixture.slice()
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [self] in self.logProbeState("eye") }
        DispatchQueue.main.asyncAfter(deadline: .now() + Flags.probeSeconds) {
            IslandLog.write("probe-eye done — quitting 0")
            NSApp.terminate(nil)
        }
    }

    func logProbeState(_ which: String) {
        let info = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
            .first { ($0[kCGWindowNumber as String] as? Int) == window.windowNumber }
        IslandLog.write("probe \(which): state=\(ui.state) shown=\(shown) visible=\(window.isVisible) key=\(window.isKeyWindow) active=\(NSApp.isActive) number=\(window.windowNumber) level=\(window.level.rawValue) sharing=\(window.sharingType.rawValue) frame=\(window.frame) drawn=\(drawnGlobal) anchor=\(ui.anchor.map { "\($0.rect) notch=\($0.notch) uuid=\(IslandGeometry.uuid(of: $0.screen))" } ?? "none") hitTestOutside=\(hosting.hitTest(NSPoint(x: 2, y: 2)) == nil)")
        IslandLog.write("probe \(which): CGWindowList bounds=\(info?[kCGWindowBounds as String] ?? "?") sharingState=\(info?[kCGWindowSharingState as String] ?? "?") layer=\(info?[kCGWindowLayer as String] ?? "?") alpha=\(info?[kCGWindowAlpha as String] ?? "?")")
        IslandLog.write("probe \(which): meeting topics=\(store.meeting?.topics.map(\.label) ?? []) brief=\(store.meeting?.current?.title ?? "none") lines=\(store.meeting?.lines.count ?? 0) frames=\(model.frames)")
    }
}

/// The island's UserDefaults keys (the same suite as `CompanionSettings`; D19 moves them out of the webview's localStorage).
enum IslandSettings {
    static var alwaysShow: Bool {
        get { Settings.defaults.bool(forKey: "islandAlwaysShow") }
        set { Settings.defaults.set(newValue, forKey: "islandAlwaysShow") }
    }
    /// Default hidden, as `meeting-settings.ts:10` had it.
    static var eyeHidden: Bool {
        get { Settings.defaults.object(forKey: "islandEyeHidden") == nil ? true : Settings.defaults.bool(forKey: "islandEyeHidden") }
        set { Settings.defaults.set(newValue, forKey: "islandEyeHidden") }
    }
    /// Display UUID → `[x, y, w, h]` in that screen's own coordinates.
    static var frames: [String: CGRect] {
        get {
            guard let raw = Settings.defaults.dictionary(forKey: "islandFrames") as? [String: [Double]] else { return [:] }
            return raw.compactMapValues { $0.count == 4 ? CGRect(x: $0[0], y: $0[1], width: $0[2], height: $0[3]) : nil }
        }
        set { Settings.defaults.set(newValue.mapValues { [$0.minX, $0.minY, $0.width, $0.height] }, forKey: "islandFrames") }
    }
}

/// The probes' meeting: recording 12:31, two topics (one cached, one from the oracle), a brief, a few lines.
enum IslandFixture {
    static func slice() -> MeetingSlice {
        var s = MeetingSlice.empty
        s.meeting = "probe-meeting"; s.session = "probe-session"; s.state = .recording
        s.elapsedMs = 12 * 60_000 + 31_000; s.clockAt = Date()
        s.topics = [Topic(id: "t1", label: "SRE-3268 hold at zero", atMs: 1_000, cached: "c1"),
                    Topic(id: "t2", label: "Flipt DR", atMs: 2_000, source: .oracle)]
        s.briefs = [Brief(id: "b1", topic: "t1", title: "DR task E.4 — hold every west workload at zero",
                          bullets: ["38 repo branches sre-3268-dr-hold-at-zero + 7 patch files on MJ-26xx PRs",
                                    "kustomize half of done_when met; in Sky's review",
                                    "Flipt DR prepared, not deployed"], source: .tap)]
        s.cursor = 0
        s.lines = [.init(utterance: Utterance(track: .system, t0Ms: 1_000, t1Ms: 4_000, text: "Let's hold the west workloads at zero until the review lands.", speaker: "s2"), open: false),
                   .init(utterance: Utterance(track: .mic, t0Ms: 5_000, t1Ms: 7_000, text: "Agreed, I'll brief the DR task."), open: false),
                   .init(utterance: Utterance(track: .system, t0Ms: 8_000, t1Ms: 9_000, text: "and Flipt is prepared but", speaker: "s1"), open: true)]
        s.level = Level(meeting: "probe-meeting", micRms: 0.04, systemRms: 0.07)
        s.degraded = [.micNoAec]
        return s
    }
}

/// Settings › Island (§3.8): Always show · position reset (this display / all) · the eye default.
struct IslandSettingsTab: View {
    @State private var alwaysShow = IslandSettings.alwaysShow
    @State private var eyeHidden = IslandSettings.eyeHidden
    @State private var frames = IslandSettings.frames.count

    var body: some View {
        Form {
            Toggle("Always show the island", isOn: $alwaysShow)
                .onChange(of: alwaysShow) { _, v in controller?.setAlwaysShow(v) ?? (IslandSettings.alwaysShow = v) }
            Text("Off: the island appears while a meeting is live and goes when it ends. On: the idle pill stays; a click opens Meetings.")
                .font(.caption).foregroundStyle(.secondary)
            Toggle(isOn: $eyeHidden) {
                HStack(spacing: 6) {
                    Image(systemName: EyeGlyph.symbol(eyeHidden)).foregroundStyle(EyeGlyph.tint(eyeHidden)).accessibilityHidden(true)
                    Text("Hide from screen sharing (best effort)")
                }
            }
            .help(EyeGlyph.label(eyeHidden)).accessibilityLabel(EyeGlyph.label(eyeHidden))
            .onChange(of: eyeHidden) { _, v in if let c = controller { c.ui.eyeHidden = v } else { IslandSettings.eyeHidden = v } }
            Text("Sets the legacy “no sharing” flag on every companion panel. Apple documents it as no longer honoured by modern capture; the ⌥⇧E eye toggles it while a meeting is live. Orange filled eye: viewers can see the island. Green slashed eye: hidden.")
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("Reset position on this display") { controller?.snapBack(); frames = IslandSettings.frames.count }
                Button("Reset on all displays") { controller?.snapBack(all: true); frames = IslandSettings.frames.count }
            }
            Text(frames == 0 ? "The island sits at the notch (or under the menu bar on a screen without one)."
                 : "\(frames) remembered position\(frames == 1 ? "" : "s"), one per display; double-clicking the island's grip also snaps it back.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private var controller: IslandController? { (NSApp.delegate as? AppDelegate)?.island }
}
