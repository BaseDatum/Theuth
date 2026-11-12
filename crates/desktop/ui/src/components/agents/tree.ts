import { THEUTH } from '@/components/tasks/helpers'
import type { RemoteAgent } from '@/components/tasks/home'
import type { AgentSummary } from "@/rpc/bindings/method.roster.output"

/** One (agent, engine) pair: `key` is unique across engines; `agent` is a local roster row, `remote` another engine's file; `on` the machine's name. */
export type Node = { name: string; key: string; agent: AgentSummary | null; remote: RemoteAgent | null; on: string | null; children: Node[] }

/**
 * The roster as a tree by `reports_to`, rooted at the built-in `theuth` (67 decision 1). An agent whose manager is
 * not on the roster, or whose chain loops back to itself, hangs off the root so nothing is hidden. Siblings sort by name.
 * 182: `remote` rows (other engines' agents) are nodes too — each engine's own `theuth` hangs off the root and its agents
 * under it by that engine's `reports_to`; `self` names this machine on the local nodes.
 */
export function tree(agents: readonly AgentSummary[], remote: readonly RemoteAgent[] = [], self: string | null = null): Node {
  const byName: Record<string, AgentSummary> = {}
  for (const a of agents) byName[a.file.name] = a
  const nodes: Record<string, Node> = {}
  const node = (name: string): Node => (nodes[name] ??= { name, key: name, agent: byName[name] ?? null, remote: null, on: self, children: [] })
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
    // The built-in root is on the roster too; hanging it under itself makes a cycle (stack overflow on sort/render).
    if (a.file.name === THEUTH) continue
    const rt = a.file.reports_to
    node(byName[rt] && !loops(rt, a.file.name) ? rt : THEUTH).children.push(node(a.file.name))
  }
  const there: Record<string, Node> = {}
  for (const r of remote) there[r.key] = { name: r.name, key: r.key, agent: null, remote: r, on: r.engineName, children: [] }
  const parentOf = (r: RemoteAgent): Node | undefined => (r.name === THEUTH ? undefined : (there[`${r.engine}/${r.reportsTo}`] ?? there[`${r.engine}/${THEUTH}`]))
  for (const r of remote) {
    const n = there[r.key] as Node
    let p = parentOf(r)
    // A loop in that engine's reports_to chain (or a manager that is itself) falls back to the root.
    for (let q = p, i = 0; q && i <= remote.length; i++) {
      if (q === n) {
        p = undefined
        break
      }
      q = q.remote ? parentOf(q.remote) : undefined
    }
    ;(p ?? root).children.push(n)
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

/** What a card or row says beyond the agent: the open-task count by home (undefined while loading) and where the agent lives. */
export type NodeInfo = { open: number | undefined; where: string }

export type Placed = { node: Node; x: number; y: number }
export type Layout = { cards: Placed[]; edges: { from: Placed; to: Placed }[]; width: number; height: number }
/** Card box; `h` includes the live-sessions strip under the card body (100 D4). */
export const CARD = { w: 200, h: 112, gx: 24, gy: 56 } as const

/**
 * Org-chart coordinates (paperclip's OrgChart layout): a parent is centred over its children, siblings sit side by
 * side with `gx` between subtrees, each level `h + gy` lower. Pure, so the chart is a map over `cards` and `edges`.
 */
export function layout(root: Node, c: { w: number; h: number; gx: number; gy: number } = CARD): Layout {
  const width = (n: Node): number => (n.children.length === 0 ? c.w : Math.max(c.w, n.children.reduce((s, k) => s + width(k), 0) + (n.children.length - 1) * c.gx))
  const cards: Placed[] = []
  const edges: Layout['edges'] = []
  const place = (n: Node, left: number, y: number): Placed => {
    const w = width(n)
    const me: Placed = { node: n, x: left + (w - c.w) / 2, y }
    cards.push(me)
    let x = left + (w - (n.children.reduce((s, k) => s + width(k), 0) + (n.children.length - 1) * c.gx)) / 2
    for (const k of n.children) {
      edges.push({ from: me, to: place(k, x, y + c.h + c.gy) })
      x += width(k) + c.gx
    }
    return me
  }
  place(root, 0, 0)
  return { cards, edges, width: width(root), height: Math.max(...cards.map((p) => p.y)) + c.h }
}
