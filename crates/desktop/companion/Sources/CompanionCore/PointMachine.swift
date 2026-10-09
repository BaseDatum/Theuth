import Foundation
import TheuthKit

// The one-to-one port of `crates/desktop/src/point/machine.rs:6-144` (design 149 §3.7, D12); the 16 behavioural tests
// live in `PointMachineTests`. C fixed the shapes and the 96 §4.1 constants.

/// The voice key's gesture machine — hold to talk, double-tap to lock — as a pure reducer over the hotkey's
/// pressed/released stream, Esc, the panel's replies and timer ticks. No clock, no I/O: the controller owns the
/// timers and executes the `Effect`s. Times are ms since process start; `t0` is the press that opened the gesture.
public enum PointMachine {
    /// Release before this is a tap, not a hold.
    public static let holdMs: UInt64 = 200
    /// A second press within this of a tap's release locks hands-free.
    public static let doubleTapMs: UInt64 = 300
    /// Notes shorter than this are discarded with `hint`.
    public static let minNoteMs: UInt64 = 800
    /// Pointer sampler rate (§4.2).
    public static let sampleHz: UInt64 = 60
    /// How long the answer card's `done` phase lingers.
    public static let doneMs: UInt64 = 6_000
    /// Shown on a missed double-tap, a too-short note, and Esc mid-gesture.
    public static let hint = "Hold to talk · double-tap to lock"

    public enum State: Equatable, Sendable {
        case idle
        case armed(t0: UInt64)
        case holding(t0: UInt64)
        case tap1(t0: UInt64, released: UInt64)
        case locked(t0: UInt64)
        case sending(t0: UInt64)
        case replying
    }

    public enum Event: Equatable, Sendable {
        case pressed, released, esc
        /// A timer armed on entering `armed`/`tap1`; `now` is the time it was armed for.
        case tick(now: UInt64)
        case panelDone, cancel
    }

    public enum Effect: Equatable, Sendable {
        /// Overlay + card + dictation at 0 ms.
        case start
        case showLock
        /// `reason` = `release` | `tap`.
        case stop(reason: String)
        /// `reason` = `esc` | `short` | `double-tap-missed` | `cancel`.
        case discard(reason: String, hint: String?)
        case conceal
        case refuse(reason: String)
    }

    /// One transition. `refused` is the standing reason (a live meeting leases the mic), answered instead of
    /// `.start` on a press from `.idle`/`.replying`.
    public static func step(_ s: State, _ e: Event, now: UInt64, refused: String?) -> (State, [Effect]) {
        switch (s, e) {
        case (.idle, .pressed), (.replying, .pressed):
            if let refused { return (s, [.refuse(reason: refused)]) }
            return (.armed(t0: now), [.start])
        case (.armed(let t0), .released):
            return elapsed(now, t0) >= holdMs ? send(t0, now, "release") : (.tap1(t0: t0, released: now), [])
        case (.armed(let t0), .tick) where elapsed(now, t0) >= holdMs:
            return (.holding(t0: t0), [])
        case (.holding(let t0), .released):
            return send(t0, now, "release")
        case (.armed, .esc), (.holding, .esc), (.locked, .esc):
            return discard("esc", hint)
        case (.tap1, .esc), (.sending, .esc):
            return discard("esc", nil)
        case (.armed, .cancel), (.holding, .cancel), (.tap1, .cancel), (.locked, .cancel):
            return discard("cancel", nil)
        case (.tap1(let t0, let released), .pressed):
            if elapsed(now, released) <= doubleTapMs { return (.locked(t0: t0), [.showLock]) }
            // The tick that should have closed the window has not landed yet: close it and treat this press as a
            // fresh gesture rather than swallow it.
            return (.armed(t0: now), [.discard(reason: "double-tap-missed", hint: hint), .start])
        case (.tap1(_, let released), .tick) where elapsed(now, released) > doubleTapMs:
            return discard("double-tap-missed", hint)
        case (.locked(let t0), .pressed):
            return send(t0, now, "tap")
        case (.sending, .panelDone):
            return (.replying, [])
        case (.sending, .cancel):
            return (.idle, [])
        case (.replying, .esc), (.replying, .panelDone), (.replying, .cancel):
            return (.idle, [.conceal])
        default:
            return (s, [])
        }
    }

    /// A note ends: send it, or discard one shorter than `minNoteMs`.
    private static func send(_ t0: UInt64, _ now: UInt64, _ reason: String) -> (State, [Effect]) {
        elapsed(now, t0) < minNoteMs ? discard("short", hint) : (.sending(t0: t0), [.stop(reason: reason)])
    }

    private static func discard(_ reason: String, _ hint: String?) -> (State, [Effect]) {
        (.idle, [.discard(reason: reason, hint: hint)])
    }

    /// `saturating_sub`: a late clock never underflows.
    private static func elapsed(_ now: UInt64, _ since: UInt64) -> UInt64 { now > since ? now - since : 0 }
}

/// 149 W2: what the Store publishes while a gesture is live (`Store.point`); nil while idle.
public struct PointSlice: Equatable, Sendable {
    public var state: PointMachine.State
    public var samples: [PointerSample]
    /// The engine's partial/final transcript so far.
    public var text: String
    public init(state: PointMachine.State = .idle, samples: [PointerSample] = [], text: String = "") {
        self.state = state; self.samples = samples; self.text = text
    }
}
