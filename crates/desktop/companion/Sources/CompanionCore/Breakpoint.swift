import Foundation

// 130 §5.5 breakpoint sources and spike SB §1 (the double-tap seed). Pure reducers; the AppKit sources in
// `Companion/Breakpoints` only feed them.

/// App switch or ≥ 20 s typing pause → one breakpoint each. The cap timer is the policy's own `tick`.
public struct BreakpointDetector: Sendable {
    public enum Signal: Equatable, Sendable {
        /// `NSWorkspace.didActivateApplicationNotification` with the new frontmost bundle id.
        case appActivated(String?)
        /// IOKit `HIDIdleTime` (or the CG fallback), sampled at 1 Hz.
        case idle(TimeInterval)
    }

    public static let typingPause: TimeInterval = 20

    private let own: String?
    private var frontmost: String?
    private var seeded = false
    private var idleFired = false

    /// `own`: the companion's bundle id — its own panel taking front is not a switch.
    public init(own: String?) { self.own = own }

    /// True when the signal is a breakpoint.
    public mutating func feed(_ s: Signal) -> Bool {
        switch s {
        case .appActivated(let id):
            if id == own && own != nil { return false }
            defer { frontmost = id; seeded = true }
            return seeded && id != frontmost
        case .idle(let t):
            if t >= Self.typingPause {
                if idleFired { return false }
                idleFired = true
                return true
            }
            idleFired = false
            return false
        }
    }
}

/// Device-independent modifier state; the AppKit source maps `NSEvent.ModifierFlags` onto it.
public struct Modifiers: OptionSet, Sendable, Equatable {
    public let rawValue: UInt8
    public init(rawValue: UInt8) { self.rawValue = rawValue }
    public static let shift = Modifiers(rawValue: 1 << 0)
    public static let control = Modifiers(rawValue: 1 << 1)
    public static let option = Modifiers(rawValue: 1 << 2)
    public static let command = Modifiers(rawValue: 1 << 3)
    public static let function = Modifiers(rawValue: 1 << 4)
}

/// Spike SB's detector: ⌥ pressed ALONE and released twice, the second press within `window` of the first
/// release, each hold ≤ `maxHold`, nothing else in between. Identical consecutive states (auto-repeat) are
/// ignored. Feed every flagsChanged; `cancel()` on any keyDown/mouseDown.
public struct DoubleTapDetector: Sendable {
    public var window: TimeInterval = 0.300
    public var maxHold: TimeInterval = 0.300

    private enum Phase { case idle, firstDown(TimeInterval), firstUp(TimeInterval), secondDown(TimeInterval) }
    private var phase: Phase = .idle
    private var last: Modifiers = []

    public init() {}

    /// True exactly once, on the release that completes the second tap.
    public mutating func feed(_ mods: Modifiers, at t: TimeInterval) -> Bool {
        if mods == last { return false }
        let previous = last
        last = mods
        let optionAlone = mods == .option
        let released = mods.isEmpty
        switch phase {
        case .idle:
            phase = optionAlone && previous.isEmpty ? .firstDown(t) : .idle
        case .firstDown(let down):
            phase = released && t - down <= maxHold ? .firstUp(t) : .idle
        case .firstUp(let up):
            if optionAlone && previous.isEmpty {
                phase = t - up <= window ? .secondDown(t) : .firstDown(t)
            } else {
                phase = .idle
            }
        case .secondDown(let down):
            phase = .idle
            if released && t - down <= maxHold { return true }
        }
        return false
    }

    public mutating func cancel() { phase = .idle }

    public var isIdle: Bool { if case .idle = phase { return true } else { return false } }
}
