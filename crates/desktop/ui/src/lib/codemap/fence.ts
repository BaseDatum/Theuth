/** 73 §6.8 Snapshot to board — a lens as one `json-render` `Graph` fence (65 §3.1), ≤ 12 nodes, a `ref` on every node. */
import type { GraphState } from "@/rpc/bindings/method.codeindex.output"
import { anchorOf, kindGlyph, shortLabels } from './labels'
import type { ChangeLens, FitLens, FitNode, HereLens } from './types'

export type Snapshot = { lens: 'here'; data: HereLens } | { lens: 'fit'; data: FitLens } | { lens: 'change'; data: ChangeLens }

const MAX = 12
/** `graphProps` is a strict zod object with `.optional()` fields: absent keys only, never `null`. */
type Node = { id: string; label: string; state?: GraphState; ref: string }
type Edge = { from: string; to: string; label?: string; style?: 'dashed' }

const sym = (n: FitNode, state?: GraphState): Node => ({ id: n.moniker, label: `${kindGlyph(n.kind)} ${n.name}`.trim().slice(0, 80), state, ref: anchorOf(n.path, n.line_start, n.line_end) })

/** The spec object before stringify (tests run `parseSpec` on its JSON). */
export function graphSpecOf(s: Snapshot): unknown {
  let nodes: Node[]
  const edges: Edge[] = []
  if (s.lens === 'fit') {
    const f = s.data
    const callers = f.callers.slice(0, Math.max(Math.ceil((MAX - 1) / 2), MAX - 1 - f.callees.length))
    const callees = f.callees.slice(0, MAX - 1 - callers.length)
    nodes = [...callers.map((n) => sym(n, n.is_test ? 'done' : undefined)), sym(f.focus, 'active'), ...callees.map((n) => sym(n, n.is_test ? 'done' : undefined))]
    const kept: Record<string, true> = {}
    for (const n of nodes) kept[n.id] = true
    for (const e of f.edges) {
      if (kept[e.from] && kept[e.to]) edges.push({ from: e.from, to: e.to, label: e.count > 1 ? `×${e.count}` : undefined, style: e.confidence < 1 ? 'dashed' : undefined })
    }
  } else if (s.lens === 'change') {
    // 71 §4.3: changed → danger, direct → active
    const files = [...s.data.changed.map((f) => ({ f, state: 'danger' as const })), ...s.data.direct.map((f) => ({ f, state: 'active' as const }))].slice(0, MAX)
    const short = shortLabels(files.map((x) => x.f.path))
    nodes = files.map(({ f, state }) => ({ id: f.path, label: short.get(f.path)!, state, ref: f.path }))
  } else {
    const best: Record<string, number> = {}
    for (const r of s.data.rows) best[r.path] = Math.max(best[r.path] ?? 0, r.rank)
    const paths = Object.keys(best).sort((a, b) => best[b]! - best[a]!).slice(0, MAX)
    const short = shortLabels(paths)
    nodes = paths.map((p) => ({ id: p, label: short.get(p)!, ref: p }))
  }
  return { root: 'g', elements: { g: { type: 'Graph', props: { nodes, edges } } } }
}

/** The fenced markdown body. */
export function toGraphFence(s: Snapshot): string {
  return '```json-render\n' + JSON.stringify(graphSpecOf(s)) + '\n```'
}
