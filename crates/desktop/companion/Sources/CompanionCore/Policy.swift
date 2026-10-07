import Foundation
import TheuthKit

// 130 §4.4 (the policy table) and §5.5 (the state machine). Pure: items in, banners out; the app posts them.

/// The four rows of the §4.4 table.
public enum NotificationClass: String, Sendable, CaseIterable {
    case blocking, deferrable, error, never
}

/// Per-class user preference (Settings › Notifications).
public enum NotifyMode: String, Sendable, Codable {
    case immediate, atBreak, off
}

public struct NotificationPrefs: Equatable, Sendable {
    public var blocking: NotifyMode = .immediate
    public var deferrable: NotifyMode = .atBreak
    public var error: NotifyMode = .immediate
    public var soundOnErrors = true
    /// R1-4: the restricted entitlement needs a provisioning profile; false until the release Mac has one.
    public var timeSensitiveAvailable = false

    public init() {}

    public func mode(for c: NotificationClass) -> NotifyMode {
        switch c {
        case .blocking: return blocking
        case .deferrable: return deferrable
        case .error: return error
        case .never: return .off
        }
    }
}

/// One `UNNotificationRequest` the app should post.
public struct Banner: Equatable, Sendable {
    /// Registered categories (D8); `error` and the batch's reuse for plain kinds are W2 additions.
    public enum Category: String, Sendable, CaseIterable { case question, approval, review, finished, batch, error }
    public enum Level: String, Sendable { case active, passive, timeSensitive }

    public var id: String
    public var category: Category
    public var title: String
    public var body: String
    /// `threadIdentifier`: the agent, so an agent's banners stack.
    public var thread: String
    public var level: Level
    public var sound: Bool
    /// The items it covers (one, or the batch); withdrawn when they are all gone.
    public var itemIds: [String]

    public init(id: String, category: Category, title: String, body: String, thread: String, level: Level, sound: Bool, itemIds: [String]) {
        self.id = id; self.category = category; self.title = title; self.body = body; self.thread = thread
        self.level = level; self.sound = sound; self.itemIds = itemIds
    }

    public static let floodId = "flood"
    public static let engineId = "engine"
}

public struct PolicyOutput: Equatable, Sendable {
    public var post: [Banner] = []
    public var withdraw: [String] = []
    public init() {}
    public var isEmpty: Bool { post.isEmpty && withdraw.isEmpty }
    mutating func merge(_ o: PolicyOutput) { post += o.post; withdraw += o.withdraw }
}

public struct NotificationPolicy: Sendable {
    /// Held deferrables go out at the latest this long after the first one.
    public static let cap: TimeInterval = 120
    /// Steady state: at most one deferrable banner per window.
    public static let rate: TimeInterval = 120
    /// More than this many needs-you items → flood mode.
    public static let floodThreshold = 10

    public var prefs: NotificationPrefs
    public private(set) var pausedUntil: Date?

    private var primed = false
    private var known: Set<String> = []
    private var held: [AttentionItem] = []
    private var holdingSince: Date?
    private var breakpointPending = false
    private var lastDeferrableAt: Date?
    private var flood = false
    private var floodShown = 0
    private var waitingNow = 0
    private var engineLost = false
    /// Posted banner id → item ids still waiting.
    private var live: [String: Set<String>] = [:]

    public init(prefs: NotificationPrefs = NotificationPrefs()) { self.prefs = prefs }

    public var heldCount: Int { held.count }
    public var inFlood: Bool { flood }
    public func isPaused(now: Date) -> Bool { pausedUntil.map { now < $0 } ?? false }

    // MARK: classification (§4.4)

    public static func classify(_ item: AttentionItem) -> NotificationClass {
        switch item.kind {
        case .approval, .ask, .decision, .secret, .handoff: return .blocking
        case .review, .hire, .finished, .blocked: return .deferrable
        // `attention.rs:548-551`: "paused at the cap; …" vs "at 80 %: the cap pauses it".
        case .budget: return item.detail?.hasPrefix("paused") == true ? .error : .deferrable
        case .node: return .error
        case .show, .resume, .other: return .never
        }
    }

    public static func category(for item: AttentionItem) -> Banner.Category {
        switch item.kind {
        case .ask, .decision: return .question
        case .approval: return .approval
        case .review: return .review
        case .finished: return .finished
        case .node: return .error
        case .budget: return classify(item) == .error ? .error : .batch
        default: return .batch
        }
    }

    // MARK: inputs

    /// Items from `attention_list`; the first call seeds the known set without posting (those were
    /// waiting before the companion started) and only takes the flood reading.
    public mutating func update(items: [AttentionItem], now: Date) -> PolicyOutput {
        var out = PolicyOutput()
        let ids = Set(items.map(\.id))
        for (bid, set) in live where !set.isEmpty {
            let left = set.intersection(ids)
            if left.isEmpty { out.withdraw.append(bid); live[bid] = nil } else { live[bid] = left }
        }
        held.removeAll { !ids.contains($0.id) }
        if held.isEmpty { holdingSince = nil; breakpointPending = false }
        let fresh = items.filter { !known.contains($0.id) }
        known = ids
        let waiting = items.filter { $0.kind.needsYou }.count
        waitingNow = waiting

        if !primed {
            primed = true
            flood = waiting > Self.floodThreshold
            floodShown = flood ? waiting : 0
            return out
        }
        if isPaused(now: now) { return out }

        if waiting > Self.floodThreshold {
            if !flood {
                flood = true
                held = []; holdingSince = nil; breakpointPending = false
                out.post.append(floodBanner(waiting))
            }
        } else if flood {
            flood = false
            floodShown = 0
            if live[Banner.floodId] != nil { live[Banner.floodId] = nil; out.withdraw.append(Banner.floodId) }
        }

        for item in fresh {
            let cls = Self.classify(item)
            // Flood: no per-item banners except errors.
            if flood && cls != .error { continue }
            switch prefs.mode(for: cls) {
            case .off: continue
            case .immediate:
                out.post.append(banner(item, cls: cls))
            case .atBreak:
                held.append(item)
                if holdingSince == nil { holdingSince = now }
            }
        }
        out.merge(release(now: now, breakpoint: false))
        return out
    }

    /// An app switch or a ≥ 20 s typing pause (`BreakpointDetector`).
    public mutating func breakpoint(now: Date) -> PolicyOutput {
        var out = PolicyOutput()
        if isPaused(now: now) { return out }
        if flood, let waiting = floodWaiting, waiting != floodShown {
            out.post.append(floodBanner(waiting))
        }
        out.merge(release(now: now, breakpoint: true))
        return out
    }

    /// The 1 Hz `.common` timer: the 2-minute cap, the rate window, the end of a pause.
    public mutating func tick(now: Date) -> PolicyOutput {
        if let until = pausedUntil, now >= until { pausedUntil = nil }
        if isPaused(now: now) { return PolicyOutput() }
        return release(now: now, breakpoint: false)
    }

    /// Menu: Pause 1 h / until tomorrow; `nil` resumes. Nothing is posted while paused; the rail is live.
    public mutating func pause(until: Date?) { pausedUntil = until }

    /// The RPC connection dropped (error class) / came back (withdraw).
    public mutating func engine(connected: Bool, now: Date) -> PolicyOutput {
        var out = PolicyOutput()
        if connected {
            engineLost = false
            if live[Banner.engineId] != nil { live[Banner.engineId] = nil; out.withdraw.append(Banner.engineId) }
            return out
        }
        guard !engineLost else { return out }
        engineLost = true
        guard !isPaused(now: now), prefs.error != .off else { return out }
        let b = Banner(id: Banner.engineId, category: .error, title: "theuth engine unreachable",
                       body: "The companion lost the engine; reconnecting.", thread: "theuth",
                       level: errorLevel, sound: prefs.soundOnErrors, itemIds: [])
        live[b.id] = []
        out.post.append(b)
        return out
    }

    // MARK: internals

    private var floodWaiting: Int? { flood ? waitingNow : nil }

    private var errorLevel: Banner.Level { prefs.timeSensitiveAvailable ? .timeSensitive : .active }

    private mutating func release(now: Date, breakpoint: Bool) -> PolicyOutput {
        var out = PolicyOutput()
        if breakpoint { breakpointPending = true }
        guard !held.isEmpty, let since = holdingSince else { return out }
        guard breakpointPending || now.timeIntervalSince(since) >= Self.cap else { return out }
        // §5.5 last-but-one row: hold even at a breakpoint until the rate window passes.
        if let last = lastDeferrableAt, now.timeIntervalSince(last) < Self.rate { return out }
        let b = held.count == 1 ? banner(held[0], cls: Self.classify(held[0])) : batchBanner(held, now: now)
        out.post.append(b)
        held = []; holdingSince = nil; breakpointPending = false
        lastDeferrableAt = now
        return out
    }

    private mutating func banner(_ item: AttentionItem, cls: NotificationClass) -> Banner {
        let who = item.agent ?? item.session.map { String($0.prefix(8)) } ?? "theuth"
        let level: Banner.Level
        let sound: Bool
        switch cls {
        case .error: level = errorLevel; sound = prefs.soundOnErrors
        case .deferrable: level = .passive; sound = false
        default: level = .active; sound = false
        }
        let b = Banner(id: item.id, category: Self.category(for: item), title: "\(who) · \(Self.label(item))",
                       body: item.title, thread: who, level: level, sound: sound, itemIds: [item.id])
        live[b.id] = [item.id]
        return b
    }

    private mutating func batchBanner(_ items: [AttentionItem], now: Date) -> Banner {
        let finished = items.filter { $0.kind == .finished }.count
        let later = items.count - finished
        var parts: [String] = []
        if later > 0 { parts.append("\(later) for later") }
        if finished > 0 { parts.append("\(finished) finished") }
        let b = Banner(id: "batch-\(UInt64(now.timeIntervalSince1970 * 1000))", category: .batch,
                       title: "theuth at your break", body: parts.joined(separator: " · "), thread: "theuth",
                       level: .passive, sound: false, itemIds: items.map(\.id))
        live[b.id] = Set(b.itemIds)
        return b
    }

    private mutating func floodBanner(_ waiting: Int) -> Banner {
        floodShown = waiting
        let b = Banner(id: Banner.floodId, category: .batch, title: "theuth",
                       body: "\(waiting) agents need you — open the panel", thread: "theuth",
                       level: .active, sound: false, itemIds: [])
        live[b.id] = []
        return b
    }

    static func label(_ item: AttentionItem) -> String {
        switch item.kind {
        case .approval: return "approval"
        case .ask: return "question"
        case .decision: return "decision"
        case .secret: return "secret"
        case .handoff: return "needs you at its desktop"
        case .review: return "review"
        case .hire: return "hire"
        case .finished: return "finished"
        case .blocked: return "blocked"
        case .budget: return classify(item) == .error ? "paused · budget" : "budget 80 %"
        case .node: return "unreachable"
        case .show: return "show"
        case .resume: return "resume"
        case .other(let s): return s
        }
    }
}
