import { expect, test } from 'vitest'
import change from '@/components/codemap/__fixtures__/change.json'
import fit from '@/components/codemap/__fixtures__/fit.json'
import here from '@/components/codemap/__fixtures__/here.json'
import { parseSpec } from '@/components/chat/surface/parse'
import { graphSpecOf, toGraphFence, type Snapshot } from './fence'
import type { ChangeLens, FitLens, HereLens } from './types'

const snaps: Snapshot[] = [
  { lens: 'here', data: here as HereLens },
  { lens: 'fit', data: fit as FitLens },
  { lens: 'change', data: change as unknown as ChangeLens },
]
type Graph = { nodes: { id: string; label: string; ref?: string; state?: string }[]; edges: { from: string; to: string; label?: string; style?: string }[] }
const graphOf = (s: Snapshot) => (graphSpecOf(s) as { elements: { g: { props: Graph } } }).elements.g.props

test('every lens snapshot is a parseSpec-valid Graph fence with ≤ 12 nodes, a ref on each, labels ≤ 80', () => {
  for (const s of snaps) {
    const r = parseSpec(JSON.stringify(graphSpecOf(s)))
    expect(r, s.lens).toMatchObject({ repaired: false, spec: { root: 'g' } })
    const g = graphOf(s)
    expect(g.nodes.length, s.lens).toBeLessThanOrEqual(12)
    expect(g.nodes.length, s.lens).toBeGreaterThan(0)
    for (const n of g.nodes) {
      expect(n.ref, n.id).toBeTruthy()
      expect(n.label.length).toBeLessThanOrEqual(80)
    }
    expect(toGraphFence(s)).toBe('```json-render\n' + JSON.stringify(graphSpecOf(s)) + '\n```')
  }
})

test('fit: focus active with a line-range ref, edges only between kept nodes, ×N / dashed carried', () => {
  const f = fit as FitLens
  const g = graphOf(snaps[1]!)
  expect(g.nodes.find((n) => n.id === f.focus.moniker)).toMatchObject({ state: 'active', ref: 'crates/tools/src/registry.rs#L1366-L1439', label: 'ƒ ToolRegistry::start' })
  const ids = new Set(g.nodes.map((n) => n.id))
  for (const e of g.edges) expect(ids.has(e.from) && ids.has(e.to), `${e.from}→${e.to}`).toBe(true)
  expect(g.edges.some((e) => e.style === 'dashed')).toBe(true)
  expect(g.edges.some((e) => e.label === '×2')).toBe(true)
  expect(g.nodes.some((n) => f.callees.some((c) => c.moniker === n.id))).toBe(true)
})

test('change: changed → danger then direct → active, no edges; here: files by best rank, no edges', () => {
  const c = graphOf(snaps[2]!)
  expect(c.edges).toEqual([])
  expect(c.nodes.slice(0, 3).map((n) => n.state)).toEqual(['danger', 'danger', 'danger'])
  expect(c.nodes.slice(3).every((n) => n.state === 'active')).toBe(true)
  expect(c.nodes.length).toBe(12)
  const h = graphOf(snaps[0]!)
  expect(h.edges).toEqual([])
  const rows = (here as HereLens).rows
  const best = (p: string) => Math.max(...rows.filter((r) => r.path === p).map((r) => r.rank))
  const ranks = h.nodes.map((n) => best(n.id))
  expect(ranks).toEqual([...ranks].sort((a, b) => b - a))
  expect(h.nodes[0]!.ref).toBe(h.nodes[0]!.id)
})
