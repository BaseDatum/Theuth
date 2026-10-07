import Foundation

/// `tasks::activity::ActivityEntry` (`crates/tasks/src/activity.rs:94-111`); `kind` is kept as its wire word.
public struct ActivityEntry: Codable, Equatable, Sendable {
    public var id: Int64
    public var tsMs: UInt64
    public var kind: String
    /// `agent:<name>` | `user` | `user@<client>` (E4) | `system`.
    public var actor: String
    public var subject: String
    public var summary: String
    public var cost: Double?

    public init(id: Int64, tsMs: UInt64, kind: String, actor: String, subject: String, summary: String, cost: Double? = nil) {
        self.id = id; self.tsMs = tsMs; self.kind = kind; self.actor = actor; self.subject = subject; self.summary = summary; self.cost = cost
    }
}

/// `activity_list`'s one positional argument (`activity.rs:117-125`; `kinds: null` = every kind).
public struct ActivityFilter: Encodable, Equatable, Sendable {
    public var sinceMs: UInt64?
    public var kinds: [String]?
    public var limit: UInt32?
    public init(sinceMs: UInt64?, kinds: [String]? = nil, limit: UInt32? = nil) {
        self.sinceMs = sinceMs; self.kinds = kinds; self.limit = limit
    }
}

/// One line in the Done zone: an ask handed to theuth, or a keystroke answer the engine recorded (E4).
public struct Receipt: Equatable, Identifiable, Sendable {
    public enum Kind: Equatable, Sendable { case ask, answer }

    /// `activity:<row id>` for engine entries, `ask:<uuid>` for local asks.
    public let id: String
    public var kind: Kind
    public var at: Date
    /// "theuth" for asks; the subject's agent or kind word for answers.
    public var who: String
    public var text: String
    /// `theuth://c/<session>` target when known.
    public var session: String?

    public init(id: String, kind: Kind, at: Date, who: String, text: String, session: String? = nil) {
        self.id = id; self.kind = kind; self.at = at; self.who = who; self.text = text; self.session = session
    }

    public static let companionActor = "user@companion"

    /// Keystroke answers only: `actor = user@companion` (W0 E4). Other actors are not this surface's receipts.
    public init?(activity e: ActivityEntry) {
        guard e.actor == Self.companionActor else { return nil }
        self.init(id: "activity:\(e.id)", kind: .answer, at: Date(timeIntervalSince1970: Double(e.tsMs) / 1000),
                  who: e.subject, text: e.summary)
    }
}

/// The last 24 h of receipts, newest first, deduplicated by id; `clear` hides one for good (E on the Done zone).
public struct ReceiptModel: Equatable, Sendable {
    public static let window: TimeInterval = 24 * 60 * 60

    public private(set) var entries: [Receipt] = []
    public private(set) var cleared: Set<String> = []

    public init() {}

    /// Insert or replace (same id), keep newest first, drop what is older than the window.
    public mutating func add(_ r: Receipt, now: Date) {
        entries.removeAll { $0.id == r.id }
        entries.append(r)
        prune(now: now)
    }

    public mutating func merge(activity: [ActivityEntry], now: Date) {
        for e in activity { if let r = Receipt(activity: e) { entries.removeAll { $0.id == r.id }; entries.append(r) } }
        prune(now: now)
    }

    /// Streaming reply text for an ask receipt.
    public mutating func update(id: String, text: String) {
        guard let i = entries.firstIndex(where: { $0.id == id }) else { return }
        entries[i].text = text
    }

    public mutating func clear(id: String) {
        cleared.insert(id)
        entries.removeAll { $0.id == id }
    }

    public mutating func prune(now: Date) {
        let cutoff = now.addingTimeInterval(-Self.window)
        entries.removeAll { $0.at < cutoff || cleared.contains($0.id) }
        entries.sort { ($0.at, $0.id) > ($1.at, $1.id) }
    }

    public var isEmpty: Bool { entries.isEmpty }
    public var count: Int { entries.count }
}
