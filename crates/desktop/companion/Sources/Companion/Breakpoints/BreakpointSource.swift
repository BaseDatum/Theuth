import AppKit
import CompanionCore
import IOKit

/// Feeds `BreakpointDetector` (130 §5.5, R1-3): `NSWorkspace.didActivateApplicationNotification`, and IOKit
/// `HIDIdleTime` (hardware input only; `CGEventSource.secondsSinceLastEventType` when the registry entry is
/// missing) sampled by a 1 Hz `.common`-mode `Timer` — exact ticks, never `asyncAfter` (spike SB §4). The
/// same tick drives the policy's 2-minute cap. Permission-free (SB run C).
@MainActor
final class BreakpointSource {
    var onBreakpoint: (() -> Void)?
    var onTick: ((Date) -> Void)?

    private var detector = BreakpointDetector(own: Bundle.main.bundleIdentifier)
    private var observer: NSObjectProtocol?
    private var timer: Timer?

    func start() {
        observer = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] note in
            let id = (note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication)?.bundleIdentifier
            MainActor.assumeIsolated { self?.feed(.appActivated(id)) }
        }
        let t = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.feed(.idle(Self.idleSeconds()))
                self.onTick?(Date())
            }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func stop() {
        if let o = observer { NSWorkspace.shared.notificationCenter.removeObserver(o) }
        timer?.invalidate()
        observer = nil; timer = nil
    }

    private func feed(_ s: BreakpointDetector.Signal) {
        if detector.feed(s) { onBreakpoint?() }
    }

    /// IOKit `HIDIdleTime` (ns) from `IOHIDSystem`; CG's any-input counter as the fallback.
    static func idleSeconds() -> TimeInterval {
        let entry = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOHIDSystem"))
        defer { if entry != 0 { IOObjectRelease(entry) } }
        if entry != 0,
           let v = IORegistryEntryCreateCFProperty(entry, "HIDIdleTime" as CFString, kCFAllocatorDefault, 0)?
               .takeRetainedValue() as? NSNumber {
            return v.doubleValue / 1e9
        }
        return CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: CGEventType(rawValue: ~0)!)
    }
}

/// Double-tap ⌥ (D6 opt-in): a global + local `flagsChanged` monitor feeding spike SB's detector. Needs
/// Accessibility (R1-3); `enable()` asks with the system prompt — the ONE allowed prompt, from Settings.
@MainActor
final class DoubleTapSource {
    var onDoubleTap: (() -> Void)?

    private var detector = DoubleTapDetector()
    private var monitors: [Any] = []

    var installed: Bool { !monitors.isEmpty }

    /// `true` when trusted and installed; `false` when the prompt went up (the user must flip the switch and
    /// turn the toggle on again).
    @discardableResult
    func enable(prompt: Bool) -> Bool {
        let opts = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: prompt] as CFDictionary
        guard AXIsProcessTrustedWithOptions(opts) else { return false }
        install()
        return true
    }

    func install() {
        guard monitors.isEmpty else { return }
        let flags: (NSEvent) -> Void = { [weak self] e in
            MainActor.assumeIsolated { self?.feed(e) }
        }
        let cancel: (NSEvent) -> Void = { [weak self] _ in MainActor.assumeIsolated { self?.detector.cancel() } }
        if let g = NSEvent.addGlobalMonitorForEvents(matching: .flagsChanged, handler: flags) { monitors.append(g) }
        if let g = NSEvent.addGlobalMonitorForEvents(matching: [.keyDown, .leftMouseDown, .rightMouseDown], handler: cancel) { monitors.append(g) }
        if let l = NSEvent.addLocalMonitorForEvents(matching: .flagsChanged, handler: { e in flags(e); return e }) { monitors.append(l) }
    }

    func uninstall() {
        monitors.forEach(NSEvent.removeMonitor)
        monitors = []
    }

    private func feed(_ e: NSEvent) {
        let f = e.modifierFlags
        var m: Modifiers = []
        if f.contains(.shift) { m.insert(.shift) }
        if f.contains(.control) { m.insert(.control) }
        if f.contains(.option) { m.insert(.option) }
        if f.contains(.command) { m.insert(.command) }
        if f.contains(.function) { m.insert(.function) }
        if detector.feed(m, at: e.timestamp) { onDoubleTap?() }
    }
}
