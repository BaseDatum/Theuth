import Foundation

/// The hold before an answer is sent (130 §5.4): one action held at a time, `delay` seconds (default 3,
/// settings 0–5), cancelled by ⎋, fired exactly once. Pure — the caller supplies `now`.
public struct UndoQueue<Action: Equatable>: Equatable {
    public struct Held: Equatable {
        public let action: Action
        public let since: Date
    }

    public static var defaultDelay: TimeInterval { 3 }
    public private(set) var delay: TimeInterval
    public private(set) var held: Held?

    public init(delay: TimeInterval = 3) {
        self.delay = min(5, max(0, delay))
    }

    /// Hold `action`. A second keystroke while one is held displaces it: the displaced action is returned
    /// and the caller commits it at once (the receipt row says so). `delay`, when given, re-arms the queue
    /// with the current setting (clamped 0–5) for this and later holds — the queue itself is never replaced.
    @discardableResult
    public mutating func hold(_ action: Action, now: Date, delay: TimeInterval? = nil) -> Action? {
        let displaced = held?.action
        if let delay { self.delay = min(5, max(0, delay)) }
        held = Held(action: action, since: now)
        return displaced
    }

    /// ⎋ during the hold. `true` when something was taken back.
    @discardableResult
    public mutating func cancel() -> Bool {
        defer { held = nil }
        return held != nil
    }

    /// Fires the held action once its delay has elapsed; `nil` otherwise.
    public mutating func tick(now: Date) -> Action? {
        guard let h = held, now >= h.since.addingTimeInterval(delay) else { return nil }
        held = nil
        return h.action
    }

    public var fireAt: Date? { held.map { $0.since.addingTimeInterval(delay) } }

    /// Seconds left, 0 when nothing is held.
    public func remaining(now: Date) -> TimeInterval {
        guard let at = fireAt else { return 0 }
        return max(0, at.timeIntervalSince(now))
    }
}
