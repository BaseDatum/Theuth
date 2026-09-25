import { THEUTH } from '@/components/tasks/helpers'
import type { AgentSummary } from '@/rpc/bindings/AgentSummary'

export type Node = { name: string; agent: AgentSummary | null; children: Node[] }

/**
 * The roster as a tree by `reports_to`, rooted at the built-in `theuth` (67 decision 1). An agent whose manager is
 * not on the roster, or whose chain loops back to itself, hangs off the root so nothing is hidden. Siblings sort by name.
 */
export function tree(agents: readonly AgentSummary[]): Node {
  const byName: Record<string, AgentSummary> = {}
  for (const a of agents) byName[a.file.name] = a
  const nodes: Record<string, Node> = {}
  const node = (name: string): Node => (nodes[name] ??= { name, agent: byName[name] ?? null, children: [] })
  const loops = (from: string, to: string): boolean => {
    const seen: Record<string, true> = {}
    for (let p: AgentSummary | undefined = byName[from]; p && !seen[p.file.name]; p = byName[p.file.reports_to]) {
      if (p.file.name === to) return true
      seen[p.file.name] = true
    }
    return false
  }
  const root = node(THEUTH)
  for (const a of agents) {
    const rt = a.file.reports_to
    node(byName[rt] && !loops(rt, a.file.name) ? rt : THEUTH).children.push(node(a.file.name))
  }
  const sort = (n: Node) => {
    n.children.sort((x, y) => x.name.localeCompare(y.name))
    n.children.forEach(sort)
  }
  sort(root)
  return root
}

/** Budget fraction for the bar; `null` when `budget: none`. */
export function budgetUsed(a: AgentSummary): number | null {
  const b = a.file.budget
  return b.type === 'none' || b.amount <= 0 ? null : Math.min(1, a.spent_month / b.amount)
}
