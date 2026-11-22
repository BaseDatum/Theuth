// 208 §8.4: graph ↔ list joins and the words the strip and the nodes show. Pure.
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import type { AttentionItem } from '@/rpc/bindings/method.roster.output'
import { stateText } from '@/components/agents/sessions'
import { isLiveNode, type VisibleGraph } from './ttl'

const INCLUDE_MAX = 200

/** [id, parent, …, root]; [] when id is unknown. */
export function pathToRoot(nodes: ReadonlyMap<string, GraphNode>, id: string): string[] {
  const out: string[] = []
  const seen = new Set<string>()
  let cur = nodes.get(id)
  while (cur && !seen.has(cur.id)) {
    out.push(cur.id)
    seen.add(cur.id)
    cur = cur.parent === null ? undefined : nodes.get(cur.parent)
  }
  return out
}

/** id and every descendant in `nodes`. */
export function subtreeIds(nodes: ReadonlyMap<string, GraphNode>, id: string): Set<string> {
  const kids = new Map<string, string[]>()
  for (const n of nodes.values()) {
    if (n.parent === null) continue
    const list = kids.get(n.parent)
    if (list) list.push(n.id)
    else kids.set(n.parent, [n.id])
  }
  const out = new Set<string>()
  const stack = nodes.has(id) ? [id] : []
  while (stack.length > 0) {
    const cur = stack.pop() as string
    if (out.has(cur)) continue
    out.add(cur)
    stack.push(...(kids.get(cur) ?? []))
  }
  return out
}

const sessionOf = (i: AttentionItem, now: number): string | null =>
  typeof i.session === 'string' && i.session !== '' && !(typeof i.expires_ms === 'number' && i.expires_ms <= now) ? i.session : null

/** session → number of items that are not expired (expires_ms) and carry a session. */
export function attentionBySession(items: readonly AttentionItem[], now: number): Map<string, number> {
  const out = new Map<string, number>()
  for (const i of items) {
    const s = sessionOf(i, now)
    if (s !== null) out.set(s, (out.get(s) ?? 0) + 1)
  }
  return out
}

/** Sessions of those items, unique, sorted, at most 200: `include` for agent_graph. */
export function includeSessions(items: readonly AttentionItem[], now: number): string[] {
  return [...attentionBySession(items, now).keys()].sort().slice(0, INCLUDE_MAX)
}

const needsYou = (v: { node: GraphNode; attention: number }): boolean =>
  v.attention > 0 || v.node.activity.state.state === 'approval' || v.node.waits.includes('user')

/** Over visible non-peer nodes: needs you, working, finished (contract §8.4). */
export function stripCounts(view: VisibleGraph): { needYou: number; working: number; finished: number } {
  let needYou = 0
  let working = 0
  let finished = 0
  for (const v of view.nodes.values()) {
    if (v.node.kind === 'peer') continue
    if (needsYou(v)) needYou++
    else if (isLiveNode(v.node)) working++
    else if (v.node.outcome !== null) finished++
  }
  return { needYou, working, finished }
}

/** The strip's counts: `needYou` is the list's live item count; `working`/`finished` null until the graph loaded. */
export type StripCounts = { needYou: number; working: number | null; finished: number | null }

/** '34 need you · 11 working · 14 finished'; '34 need you' alone while the graph has not loaded. */
export function stripText(c: StripCounts): string {
  const need = `${c.needYou} need you`
  return c.working === null || c.finished === null ? need : `${need} · ${c.working} working · ${c.finished} finished`
}

/** root/desk/peer: title; child/observed: name ?? title. */
export function nodeLabel(node: GraphNode): string {
  return node.kind === 'child' || node.kind === 'observed' ? (node.name ?? node.title) : node.title
}

const OUTCOME_WORD = { finished: 'finished', failed: 'failed', cancelled: 'cancelled', timed_out: 'timed out' } as const

/** Row 2's first part: what the node is doing or how it ended. */
export function statusText(node: GraphNode): string {
  const s = node.activity.state
  if (s.state === 'approval' || s.state === 'awaiting') return stateText(s)
  if (node.waits.includes('user')) return 'waiting on you'
  if (node.waits.includes('job')) return 'waiting on a job'
  if (node.waits.includes('children')) return 'waiting on children'
  if (s.state === 'running') return 'running'
  if (s.state === 'error') return 'error'
  return node.outcome ? OUTCOME_WORD[node.outcome] : 'idle'
}

/** A peer address as a graph node id (B's `endpoint`): `session:<id>` → id, `peer:<name>[/…]` → `peer:<name>`, else null. */
export function endpointId(addr: string): string | null {
  if (addr.startsWith('session:')) return addr.slice('session:'.length) || null
  if (addr.startsWith('peer:')) {
    const name = addr.slice('peer:'.length).split('/')[0] ?? ''
    return name ? `peer:${name}` : null
  }
  return null
}
