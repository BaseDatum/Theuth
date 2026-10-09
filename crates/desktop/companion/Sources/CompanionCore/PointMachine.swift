import Foundation
import TheuthKit

// 149 W2 fills `step` (design 149 §3.7): the one-to-one port of `crates/desktop/src/point/machine.rs:6-69` with
// its 17 tests. C fixes the shapes and the 96 §4.1 constants.

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
        fatalError("149 W2")
    }
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
