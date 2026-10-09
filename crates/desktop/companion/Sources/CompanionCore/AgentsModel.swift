import Foundation
import TheuthKit

// 149 W1 fills the bodies (design 149 §3.2, §5.1, §6 E7 fallback rule). C fixes the shapes.

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

    /// `list_activity` ∪ `agents_list` ∪ `list_sessions` + `mesh_status`; roots before their children.
    public static func groups(activity: [AgentInfo], agents: [AgentSummary], sessions: [SessionSummary],
                              mesh: MeshStatus?, status: SessionsStatus?) -> [HostGroup] {
        fatalError("149 W1")
    }

    /// The row the panel opens scrolled to: the first working one.
    public static func firstWorking(_ groups: [HostGroup]) -> AgentRow? {
        fatalError("149 W1")
    }
}
