import AppKit
import CompanionCore

extension AppDelegate {
    /// 149 W1 (§3.1 behaviour, §3.2, §10 probes): the agents panel + its ⌃⇧A row, what the four pills and the agent dots
    /// do, and the three W1 probes. Called once from `applicationDidFinishLaunching` at the W1 insertion point.
    func installW1(rail: RailController, attention: PanelController) {
        let agents = AgentsPanelController(store: store)
        AgentsPanelController.shared = agents
        agents.installHotkeys()

        // D23: an agent dot opens the agents panel scrolled to its row; a needs-you/finished mark opens its card.
        rail.onMark = { [weak self, weak rail, weak agents] mark in
            guard let self, let rail, let agents, !AskFlags.automated else { return }
            if let id = mark.itemId {
                self.store.focus(id: id)
                attention.open(anchor: .rail(rail.frame(of: mark)))
            } else {
                agents.open(anchor: .rail(rail.frame(of: mark)), scrollTo: mark.session)
            }
        }
        // §3.1 Behaviour: inbox → the Attention panel (oldest needs-you focused, or the empty state); agents → the agents
        // panel; actions → the Ask bar; more → the status item's menu as a popup at the pill.
        rail.onPill = { [weak self, weak rail, weak agents] kind, frame in
            guard let self, let rail, let agents, !AskFlags.automated else { return }
            switch kind {
            case .inbox:
                if let oldest = self.store.rail.needsYou.first?.itemId { self.store.focus(id: oldest) }
                attention.toggle(anchor: .rail(frame))
            case .agents:
                agents.toggle(anchor: .rail(frame))
            case .actions:
                self.askBar.toggle()
            case .more:
                let menu = self.statusMenu()
                let at = NSPoint(x: Settings.railSide == "left" ? frame.maxX + 4 : frame.minX - 4, y: frame.maxY)
                menu.popUp(positioning: nil, at: at, in: nil)
                _ = rail
            }
        }

        if let s = Flags.probeRail { RailProbe.run(seconds: s, store: store, rail: rail) }
        if let s = Flags.probeAgents { AgentsProbe.run(seconds: s, store: store, agents: agents) }
        if let kind = Flags.probePanel { PanelProbe.run(kind: kind, store: store, attention: attention) }
    }
}
