import Foundation
import TheuthKit

// 149 W1 (design 149 §3.2, D2, §5.1, §6 E7 fallback rule, Revision 1 rows 15–16). C fixed the shapes.

/// One row of the agents panel: a session (root or child) or a desk, grouped by host.
public struct AgentRow: Equatable, Sendable, Identifiable {
    public var id: String
    public var session: String
    /// `SessionSummary.title`/`name` — never the harness key (recon C1.6).
    public var name: String
    /// `omp` · `opencode` · `claude` · `codex` · `theuth` · `unknown` — the icon set's key.
    public var harness: String
    public var state: AgentState
    /// Folded children (`⑂N`).
    public var children: Int
    public var last: String
    /// `""` or `mesh_status.self_id` = this Mac; else the peer's engine id.
    public var host: String
    /// `false` for a remote-owned row: hollow dot, "on ‹node›", mic and ✕ disabled (E7 fallback).
    public var live: Bool

    public init(id: String, session: String, name: String, harness: String, state: AgentState, children: Int = 0,
                last: String = "", host: String = "", live: Bool = true) {
        self.id = id; self.session = session; self.name = name; self.harness = harness; self.state = state
        self.children = children; self.last = last; self.host = host; self.live = live
    }

    /// A desk from `agents_list` (`id` = `agent:<name>`): ✕ pauses it (`agent_pause`) instead of cancelling a session.
    public var deskName: String? { id.hasPrefix(Self.deskPrefix) ? String(id.dropFirst(Self.deskPrefix.count)) : nil }
    public static let deskPrefix = "agent:"

    /// Something to stop: the session runs or waits (D3; a cancel on an idle session is "nothing is running").
    public var isActive: Bool {
        switch state {
        case .running, .awaiting, .approval: return true
        case .idle, .error, .cancelled: return false
        }
    }
}

/// Rows under a host header: "this Mac" first, then one group per other engine, named from `mesh_status.peers`.
public struct HostGroup: Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var rows: [AgentRow]
    public init(id: String, title: String, rows: [AgentRow]) { self.id = id; self.title = title; self.rows = rows }
}

public enum AgentsModel {
    public static let thisMac = "this Mac"
    /// D18: the shared icon set's keys; `nil` = theuth, anything else = `unknown`.
    public static let harnessKeys: Set<String> = ["omp", "opencode", "claude", "codex", "theuth"]

    public static func harnessKey(_ agent: String?) -> String {
        guard let a = agent, !a.isEmpty else { return "theuth" }
        return harnessKeys.contains(a) ? a : "unknown"
    }

    /// `list_activity` ∪ `agents_list` ∪ `list_sessions` + `mesh_status`; roots before their children (children are folded
    /// into `⑂N`). A row owned elsewhere has no live state on this engine (`supervisor.rs:6547-6557`): `live == false`,
    /// state `.idle`, never a guessed spinner.
    public static func groups(activity: [AgentInfo], agents: [AgentSummary], sessions: [SessionSummary],
                              mesh: MeshStatus?, status: SessionsStatus?) -> [HostGroup] {
        let selfId = mesh?.selfId ?? ""
        func hostOf(_ owner: String) -> String { owner == "" || owner == selfId ? "" : owner }
        let byId = Dictionary(activity.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        let live = sessions.filter { !$0.archived }
        let sessionById = Dictionary(live.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        // Children per root, from `list_sessions` parents and `list_activity` parents (either list may be absent).
        var childCount: [String: Int] = [:]
        var counted = Set<String>()
        for s in live { if let p = s.parent, !p.isEmpty, counted.insert(s.id).inserted { childCount[p, default: 0] += 1 } }
        for a in activity where !a.parent.isEmpty && counted.insert(a.id).inserted { childCount[a.parent, default: 0] += 1 }
        // Desks (`agents_list`) by their desk session: the row reads "‹title› · desk" and ✕ pauses the desk.
        let deskBySession = Dictionary(agents.compactMap { a in a.desk.map { ($0, a) } }, uniquingKeysWith: { a, _ in a })

        var rows: [AgentRow] = []
        var seen = Set<String>()
        func add(_ r: AgentRow) { if seen.insert(r.session.isEmpty ? r.id : r.session).inserted { rows.append(r) } }

        // Roots from `list_sessions` (the authoritative owner/title), joined with their activity.
        for s in live where (s.parent ?? "").isEmpty {
            let a = byId[s.id]
            let host = hostOf(s.owner)
            let remote = host != ""
            let desk = deskBySession[s.id]
            let base = s.title.isEmpty ? (s.name ?? a?.name ?? String(s.id.prefix(8))) : s.title
            let name = desk.map { d in "\(d.file.title.isEmpty ? d.file.name : d.file.title) · desk" } ?? base
            let state: AgentState = remote ? .idle : (a?.state ?? (desk.map(deskState) ?? (s.running ? .running : .idle)))
            let children = remote ? 0 : Int(a?.activity.children ?? UInt16(childCount[s.id] ?? 0))
            add(AgentRow(id: desk.map { "\(AgentRow.deskPrefix)\($0.file.name)" } ?? s.id, session: s.id, name: name,
                         harness: harnessKey(s.agent ?? a?.agent), state: state, children: children,
                         last: remote ? "" : (a?.last ?? ""), host: host, live: !remote))
        }
        // Roots `list_activity` knows and `list_sessions` does not (an older engine without the verb, or a race).
        for a in activity where a.parent.isEmpty && sessionById[a.id] == nil {
            let desk = deskBySession[a.id]
            let name = desk.map { d in "\(d.file.title.isEmpty ? d.file.name : d.file.title) · desk" } ?? (a.name.isEmpty ? String(a.id.prefix(8)) : a.name)
            add(AgentRow(id: desk.map { "\(AgentRow.deskPrefix)\($0.file.name)" } ?? a.id, session: a.id, name: name,
                         harness: harnessKey(a.agent), state: a.state, children: Int(a.activity.children),
                         last: a.last, host: "", live: true))
        }
        // Desks without a session yet (asleep): rows too, so "Ask theuth" has a place to land.
        for d in agents where d.status != .proposed && (d.desk.map { seen.contains($0) } != true) {
            add(AgentRow(id: "\(AgentRow.deskPrefix)\(d.file.name)", session: d.desk ?? "",
                         name: "\(d.file.title.isEmpty ? d.file.name : d.file.title) · desk", harness: "theuth",
                         state: deskState(d), children: 0, last: d.error ?? "", host: "", live: true))
        }

        rows.sort { (rank($0.state), $0.name.lowercased()) < (rank($1.state), $1.name.lowercased()) }

        // Groups: this Mac first, then peers in `mesh_status` order, then unknown owners by id.
        let peers = mesh?.peers ?? []
        let hereCount = status.map { " · \($0.here)" } ?? ""
        var groups = [HostGroup(id: "", title: thisMac + hereCount, rows: rows.filter { $0.host == "" })]
        var remoteHosts: [String] = []
        for r in rows where r.host != "" && !remoteHosts.contains(r.host) { remoteHosts.append(r.host) }
        remoteHosts.sort { a, b in
            let ia = peers.firstIndex { $0.id == a } ?? Int.max
            let ib = peers.firstIndex { $0.id == b } ?? Int.max
            return (ia, a) < (ib, b)
        }
        for host in remoteHosts {
            let name = peers.first { $0.id == host }?.name ?? String(host.prefix(8))
            let count = remoteHosts.count == 1 ? (status.map { " · \($0.elsewhere)" } ?? "") : ""
            groups.append(HostGroup(id: host, title: "on \(name)" + count, rows: rows.filter { $0.host == host }))
        }
        return groups
    }

    /// The row the panel opens scrolled to: the first working one.
    public static func firstWorking(_ groups: [HostGroup]) -> AgentRow? {
        groups.lazy.flatMap(\.rows).first { $0.live && $0.state == .running }
    }

    private static func deskState(_ d: AgentSummary) -> AgentState {
        switch d.status {
        case .working: return .running
        case .blocked: return .awaiting(from: "blocked")
        case .unreachable, .invalid: return .error
        case .idle, .paused, .proposed: return .idle
        }
    }

    private static func rank(_ s: AgentState) -> Int {
        switch s {
        case .running: return 0
        case .awaiting, .approval: return 1
        case .idle: return 2
        case .error: return 3
        case .cancelled: return 4
        }
    }
}
