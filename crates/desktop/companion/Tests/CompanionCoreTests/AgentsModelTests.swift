import Foundation
import Testing
@testable import CompanionCore
@testable import TheuthKit

// 149 W1: rows and host groups from `list_activity` ∪ `agents_list` ∪ `list_sessions` + `mesh_status` (§3.2, §6 E7 fallback).
struct AgentsModelTests {
    private let mesh = MeshStatus(selfId: "engine-mbp", selfName: "mbp",
                                  peers: [.init(id: "engine-studio", name: "studio", reachable: true)], lease: .init(holder: "engine-mbp"))

    private func session(_ id: String, _ title: String, agent: String? = "omp", parent: String? = nil, owner: String = "",
                         running: Bool = true, name: String? = nil, archived: Bool = false) -> SessionSummary {
        SessionSummary(id: id, title: title, name: name, agent: agent, parent: parent, running: running, owner: owner, archived: archived)
    }

    private func info(_ id: String, _ state: AgentState, children: UInt16 = 0, last: String = "", agent: String? = "omp", parent: String = "") -> AgentInfo {
        AgentInfo(id: id, name: "name-\(id)", parent: parent, agent: agent, state: state, last: last,
                  activity: SessionActivity(state: state, children: children, live: true))
    }

    @Test func emptyOwnerAndSelfIdAreThisMac() {
        let g = AgentsModel.groups(activity: [info("s-1", .running)],
                                   agents: [], sessions: [session("s-1", "w1", owner: ""), session("s-2", "w2", owner: "engine-mbp")],
                                   mesh: mesh, status: nil)
        #expect(g.count == 1)
        #expect(g[0].id == "" && g[0].title == AgentsModel.thisMac)
        #expect(g[0].rows.map(\.session) == ["s-1", "s-2"], "working first, then idle")
        #expect(g[0].rows.allSatisfy { $0.live })
        #expect(g[0].rows[0].state == .running, "state joined from list_activity")
        #expect(g[0].rows[1].state == .running, "no activity row: `running` from list_sessions")
    }

    @Test func remoteOwnerGroupsUnderThePeerNameAndIsNotLive() {
        let g = AgentsModel.groups(activity: [], agents: [],
                                   sessions: [session("s-1", "local"), session("s-9", "134-W7 WinUI rail", agent: "opencode", owner: "engine-studio", running: true)],
                                   mesh: mesh, status: nil)
        #expect(g.map(\.id) == ["", "engine-studio"], "this Mac first")
        #expect(g[1].title == "on studio")
        let r = try! #require(g[1].rows.first)
        #expect(!r.live, "no live state on this engine (E7 fallback): mic and ✕ disabled")
        #expect(r.state == .idle, "never a guessed spinner even though `running` says true elsewhere")
        #expect(r.host == "engine-studio" && r.harness == "opencode" && r.name == "134-W7 WinUI rail")
        #expect(!r.isActive)
    }

    @Test func unknownPeerIdIsNamedByItsId() {
        let g = AgentsModel.groups(activity: [], agents: [], sessions: [session("s-5", "x", owner: "engine-0123456789")], mesh: mesh, status: nil)
        #expect(g.last?.title == "on engine-0", "the id's first 8 characters when mesh_status does not know the peer")
        let noMesh = AgentsModel.groups(activity: [], agents: [], sessions: [session("s-5", "x", owner: "engine-studio")], mesh: nil, status: nil)
        #expect(noMesh.last?.title == "on engine-s")
        #expect(noMesh.first?.rows.isEmpty == true, "this Mac stays as the first group even when empty")
    }

    @Test func childrenFoldUnderTheirRootWithACount() {
        let g = AgentsModel.groups(activity: [info("s-3", .running, children: 2), info("s-3a", .running, parent: "s-3"), info("s-3b", .running, parent: "s-3")],
                                   agents: [], sessions: [session("s-3", "w3"), session("s-3a", "", parent: "s-3"), session("s-3b", "", parent: "s-3")],
                                   mesh: mesh, status: nil)
        #expect(g[0].rows.map(\.session) == ["s-3"], "children are folded, not rows")
        #expect(g[0].rows[0].children == 2)
        // Without list_activity the parents in list_sessions count.
        let g2 = AgentsModel.groups(activity: [], agents: [], sessions: [session("s-3", "w3"), session("s-3a", "", parent: "s-3")], mesh: mesh, status: nil)
        #expect(g2[0].rows[0].children == 1)
        // Without list_sessions (older engine) the activity parents count.
        let g3 = AgentsModel.groups(activity: [info("s-3", .running, children: 3), info("s-3a", .running, parent: "s-3")], agents: [], sessions: [], mesh: nil, status: nil)
        #expect(g3[0].rows.map(\.session) == ["s-3"] && g3[0].rows[0].children == 3)
    }

    @Test func desksAppearAsRows() {
        let desk = AgentSummary(file: AgentFile(name: "theuth", title: "theuth"), status: .working, desk: "s-d")
        let asleep = AgentSummary(file: AgentFile(name: "docs", title: "Docs writer"), status: .paused, desk: nil)
        let g = AgentsModel.groups(activity: [info("s-d", .running, agent: nil)], agents: [desk, asleep],
                                   sessions: [session("s-d", "theuth", agent: nil)], mesh: mesh, status: nil)
        let rows = g[0].rows
        #expect(rows.map(\.name) == ["theuth · desk", "Docs writer · desk"])
        #expect(rows[0].id == "agent:theuth" && rows[0].deskName == "theuth" && rows[0].session == "s-d")
        #expect(rows[0].harness == "theuth", "`agent` nil = theuth (D18)")
        #expect(rows[1].session == "" && rows[1].state == .idle, "an asleep desk has no session yet")
    }

    @Test func labelIsTitleOrNameNeverTheHarnessKey() {
        let g = AgentsModel.groups(activity: [info("s-1", .running, agent: "codex"), info("s-2", .idle, agent: "weird-harness")],
                                   agents: [], sessions: [session("s-1", "", agent: "codex", name: "w1"), session("s-2", "", agent: "weird-harness", name: nil)],
                                   mesh: mesh, status: nil)
        let r = g[0].rows
        #expect(r[0].name == "w1" && r[0].harness == "codex", "title empty → name; the harness is the icon, never the label")
        #expect(r[1].name == "name-s-2", "no title, no name → the activity row's name")
        #expect(r[1].harness == "unknown", "an unknown harness key maps to the `unknown` icon")
        let bare = AgentsModel.groups(activity: [], agents: [], sessions: [session("0192c5e0-7a1b", "", agent: "omp", name: nil)], mesh: nil, status: nil)
        #expect(bare[0].rows[0].name == "0192c5e0", "last resort: the short id, still not the harness key")
    }

    @Test func firstWorkingRowIsTheOpeningScrollTarget() {
        let g = AgentsModel.groups(activity: [info("s-1", .idle), info("s-2", .running)], agents: [],
                                   sessions: [session("s-1", "a"), session("s-2", "b"), session("s-9", "remote", owner: "engine-studio")],
                                   mesh: mesh, status: nil)
        #expect(AgentsModel.firstWorking(g)?.session == "s-2")
        let none = AgentsModel.groups(activity: [info("s-1", .idle)], agents: [], sessions: [session("s-1", "a")], mesh: mesh, status: nil)
        #expect(AgentsModel.firstWorking(none) == nil)
        #expect(AgentsModel.firstWorking([]) == nil)
        #expect(AgentsModel.groups(activity: [], agents: [], sessions: [session("s-x", "old", archived: true)], mesh: nil, status: nil)[0].rows.isEmpty, "archived sessions are not rows")
    }

    @Test func sessionsStatusCountsLabelTheGroups() {
        let status = SessionsStatus(engine: "engine-mbp", here: 2, elsewhere: 1, logSeq: 1, lastSnapshot: nil)
        let g = AgentsModel.groups(activity: [], agents: [],
                                   sessions: [session("s-1", "a"), session("s-2", "b"), session("s-9", "r", owner: "engine-studio")],
                                   mesh: mesh, status: status)
        #expect(g.map(\.title) == ["this Mac · 2", "on studio · 1"], "sessions_status labels the groups (§12.1 item 15), it is not per-session")
        let two = AgentsModel.groups(activity: [], agents: [],
                                     sessions: [session("s-9", "r", owner: "engine-studio"), session("s-8", "q", owner: "engine-other")],
                                     mesh: mesh, status: status)
        #expect(two.map(\.title) == ["this Mac · 2", "on studio", "on engine-o"], "`elsewhere` is one number: shown only when one remote group holds it all")
    }
}
