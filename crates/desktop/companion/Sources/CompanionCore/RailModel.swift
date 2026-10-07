import Foundation
import TheuthKit

/// One glyph in the rail (130 §4.1 table). Colour and shape both carry the state.
public enum MarkState: Equatable, Sendable {
    case needsYou(AttentionKind)
    case working, idle, finished, error, lapsed
}

public struct Mark: Equatable, Identifiable, Sendable {
    public var id: String
    public var state: MarkState
    /// Agent name, or the item's title when it has no agent.
    public var label: String
    /// One line for the tooltip / VoiceOver: "<label>, <state>, <since>".
    public var detail: String
    /// Waiting since (needs-you, finished) or running for (working).
    public var since: Date?
    /// The attention item behind a needs-you / finished mark.
    public var itemId: String?
    public var session: String?

    public var isNeedsYou: Bool { if case .needsYou = state { return true } else { return false } }
}

public struct Rail: Equatable, Sendable {
    /// Zone 1, oldest first.
    public var needsYou: [Mark]
    /// Zone 2: working · idle · finished · error · lapsed.
    public var agents: [Mark]
    /// Marks beyond `RailModel.maxMarks`, shown as `+N`.
    public var overflow: Int
    public var needsYouCount: Int
    public var workingCount: Int
    /// Nothing needs you and nothing is working: the 4 pt sliver after 8 s.
    public var quiet: Bool { needsYouCount == 0 && workingCount == 0 }
    public var marks: [Mark] { needsYou + agents }

    public init(needsYou: [Mark], agents: [Mark], overflow: Int, needsYouCount: Int, workingCount: Int) {
        self.needsYou = needsYou; self.agents = agents; self.overflow = overflow
        self.needsYouCount = needsYouCount; self.workingCount = workingCount
    }

    public static let empty = Rail(needsYou: [], agents: [], overflow: 0, needsYouCount: 0, workingCount: 0)
}

/// Settings › Rail "Rail presence" (W6, R2-3). `sliverWhenQuiet` is W1's behaviour; `alwaysSliver` leaves the
/// menu bar's gold ●N as the only indicator and the rail at 4 pt unless the pointer is over it.
public enum RailPresence: String, CaseIterable, Codable, Sendable {
    case expanded, sliverWhenQuiet, alwaysSliver

    public var title: String {
        switch self {
        case .expanded: return "Expanded"
        case .sliverWhenQuiet: return "Sliver when quiet"
        case .alwaysSliver: return "Always a sliver"
        }
    }
}

/// The collapse rule the rail controller runs; pure so the three presences are testable.
public enum RailCollapse {
    public static let width: Double = 26
    public static let sliverWidth: Double = 4
    /// Quiet this long before the sliver (`sliverWhenQuiet`).
    public static let quietDelay: TimeInterval = 8
    /// The pointer left an `alwaysSliver` rail: it stays expanded this long.
    public static let hoverLinger: TimeInterval = 1

    /// `nil`: stay expanded. Else collapse after this many seconds (0 = now). `afterHover`: the pointer just left.
    public static func collapseDelay(_ p: RailPresence, quiet: Bool, hovering: Bool, afterHover: Bool) -> TimeInterval? {
        if hovering { return nil }
        switch p {
        case .expanded: return nil
        case .sliverWhenQuiet: return quiet ? quietDelay : nil
        case .alwaysSliver: return afterHover ? hoverLinger : 0
        }
    }

    /// A collapsed rail expands when something needs you or works only in `sliverWhenQuiet`.
    public static func expandsOnActivity(_ p: RailPresence) -> Bool { p == .sliverWhenQuiet }

    public static func width(collapsed: Bool) -> Double { collapsed ? sliverWidth : width }
}

public enum RailModel {
    public static let maxMarks = 12

    /// `railState(attention, agents, activity, now)` of 130 §5.2.
    public static func derive(items: [AttentionItem], agents: [AgentSummary], activity: [AgentInfo], now: Date) -> Rail {
        let needs = items.filter { $0.kind.needsYou }
            .sorted { ($0.waitingSince, $0.id) < ($1.waitingSince, $1.id) }
            .map { item in
                Mark(id: item.id, state: .needsYou(item.kind), label: item.agent ?? item.title,
                     detail: "\(item.agent ?? item.title), \(item.kind.rawValue), \(ago(item.waitingSince, now))",
                     since: item.waitingSince, itemId: item.id, session: item.session)
            }

        var zone: [Mark] = []
        var deskSessions = Set<String>()
        for a in agents {
            if let d = a.desk { deskSessions.insert(d) }
            let state: MarkState
            switch a.status {
            case .working: state = .working
            case .idle, .blocked: state = .idle
            case .paused: state = .lapsed
            case .unreachable, .invalid: state = .error
            case .proposed: continue // already a `hire:` item in zone 1
            }
            zone.append(Mark(id: "agent:\(a.file.name)", state: state, label: a.file.name,
                             detail: "\(a.file.name), \(word(state)), \(a.file.model ?? "")", since: nil,
                             itemId: nil, session: a.desk))
        }
        for s in activity where s.activity.live && !deskSessions.contains(s.id) {
            let state: MarkState
            switch s.activity.state {
            case .running: state = .working
            case .error: state = .error
            case .idle, .awaiting, .approval, .cancelled: state = .idle
            }
            let label = s.agent ?? s.model
            let since = s.activity.turnStartedAt
            zone.append(Mark(id: "session:\(s.id)", state: state, label: label,
                             detail: "\(label), \(word(state))" + (since.map { ", running \(ago($0, now))" } ?? ""),
                             since: since, itemId: nil, session: s.id))
        }
        for f in items.filter({ $0.kind == .finished }).sorted(by: { $0.waitingSince < $1.waitingSince }) {
            zone.append(Mark(id: f.id, state: .finished, label: f.agent ?? f.title,
                             detail: "\(f.agent ?? f.title), finished, \(ago(f.waitingSince, now))",
                             since: f.waitingSince, itemId: f.id, session: f.session))
        }
        zone = zone.enumerated()
            .sorted { (rank($0.element.state), $0.offset) < (rank($1.element.state), $1.offset) }
            .map(\.element)

        let working = zone.filter { $0.state == .working }.count
        let total = needs.count + zone.count
        let overflow = max(0, total - maxMarks)
        let keepNeeds = Array(needs.prefix(maxMarks))
        let keepZone = Array(zone.prefix(max(0, maxMarks - keepNeeds.count)))
        return Rail(needsYou: keepNeeds, agents: keepZone, overflow: overflow,
                    needsYouCount: needs.count, workingCount: working)
    }

    private static func rank(_ s: MarkState) -> Int {
        switch s {
        case .working: return 0
        case .idle: return 1
        case .finished: return 2
        case .error: return 3
        case .lapsed: return 4
        case .needsYou: return -1
        }
    }

    private static func word(_ s: MarkState) -> String {
        switch s {
        case .working: return "working"
        case .idle: return "idle"
        case .finished: return "finished"
        case .error: return "error"
        case .lapsed: return "lapsed"
        case .needsYou(let k): return k.rawValue
        }
    }

    /// "40 s" · "4 m" · "2 h" · "3 d".
    public static func ago(_ from: Date, _ now: Date) -> String {
        let s = max(0, Int(now.timeIntervalSince(from)))
        if s < 60 { return "\(s) s" }
        if s < 3600 { return "\(s / 60) m" }
        if s < 86400 { return "\(s / 3600) h" }
        return "\(s / 86400) d"
    }
}
