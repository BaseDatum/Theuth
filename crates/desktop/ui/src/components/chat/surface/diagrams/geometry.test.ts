// 123 §5.4 #2/#5, §9 (closer): the real ELK (elkjs bundled, same options the worker runs) lays out every valid
// fixture of the ELK family; then no edge segment runs through a unit box it does not end at, and no edge-label
// mask (placed as `EdgePath` places it: `edgeLabelAt` + `LABEL_GAP`) overlaps a node box.
import { readdirSync, readFileSync } from 'node:fs'
import type { ElkNode } from 'elkjs/lib/elk-api'
import ELK from 'elkjs/lib/elk.bundled.js'
import { expect, test } from 'vitest'
import { maskRect, truncate } from '@/lib/diagram/label'
import type { Point } from '@/lib/diagram/types'
import { flattenElk, type Layout, toElk as graphToElk } from '@/lib/graph-layout'
import { edgeLabelAt } from '../Graph'
import { parseSpec } from '../parse'
import { toElk as entitiesToElk } from './Entities'
import { union } from './GraphDelta'
import { toElk as stateToElk } from './StateMachine'
import { toElk as swimlaneToElk } from './Swimlane'
import { toElk as treeToElk } from './TreeDiagram'

const RUST = new URL('../../../../../../../surface/fixtures/json-render/', import.meta.url)

/** Component → its ELK graph builder (`null` when this element does not lay out with ELK). */
const BUILD: Record<string, (p: never) => ElkNode | null> = {
  Graph: graphToElk,
  StateMachine: stateToElk,
  Entities: entitiesToElk,
  Swimlane: swimlaneToElk,
  GraphDelta: (p: Parameters<typeof union>[0]) => graphToElk(union(p)),
  Tree: (p: Parameters<typeof treeToElk>[0]) => (p.layout === 'nodes' || p.layout === 'org' ? treeToElk(p) : null),
}

/** Every `[fixture, type, elk graph]` of the ELK family across the valid fixtures. */
const CASES: [string, string, ElkNode][] = []
for (const f of readdirSync(RUST).filter((n) => n.startsWith('valid-') && n.endsWith('.json')).sort()) {
  const spec = parseSpec(readFileSync(new URL(f, RUST), 'utf8'))
  if ('error' in spec) throw new Error(`${f}: ${spec.error}`)
  for (const el of Object.values(spec.spec.elements)) {
    // `parseSpec` validated `props` against the catalog's zod mirror; the builder narrows to its own component.
    const root = BUILD[el.type]?.(el.props as never)
    if (root) CASES.push([f.slice(0, -5), el.type, root])
  }
}

const elk = new ELK()
const layouts = new Map<ElkNode, Promise<Layout>>()
/** The real layout; for `mrtree` (Tree) the drawn connectors replace ELK's sections: parent bottom → child top, as `TreeNodesSvg` draws them. */
const layoutOf = (root: ElkNode) => {
  let p = layouts.get(root)
  if (!p) {
    p = elk.layout(structuredClone(root)).then((r) => {
      const l = flattenElk(r)
      if (root.layoutOptions?.['elk.algorithm'] !== 'mrtree') return l
      const at: Record<string, Layout['nodes'][number]> = {}
      for (const n of l.nodes) at[n.id] = n
      l.edges = (root.edges ?? []).map((e) => {
        const pb = at[e.sources[0]!.slice(2)]!
        const c = at[e.targets[0]!.slice(2)]!
        const from = { x: pb.x + pb.w / 2, y: pb.y + pb.h }
        const to = { x: c.x + c.w / 2, y: c.y }
        const mid = (from.y + to.y) / 2
        return { index: Number(e.id.slice(2)), sections: [{ start: from, bends: from.x === to.x ? [] : [{ x: from.x, y: mid }, { x: to.x, y: mid }], end: to }] }
      })
      return l
    })
    layouts.set(root, p)
  }
  return p
}

type Box = { x0: number; y0: number; x1: number; y1: number }
const box = (n: Point & { w: number; h: number }, grow = 0): Box => ({ x0: n.x - grow, y0: n.y - grow, x1: n.x + n.w + grow, y1: n.y + n.h + grow })
const inside = (p: Point, b: Box) => p.x >= b.x0 && p.x <= b.x1 && p.y >= b.y0 && p.y <= b.y1
/** An axis-aligned segment meets the box interior (boxes shrunk by 1 px so a run along a border is not a hit). */
const crosses = (a: Point, b: Point, n: Box) => Math.max(a.x, b.x) > n.x0 + 1 && Math.min(a.x, b.x) < n.x1 - 1 && Math.max(a.y, b.y) > n.y0 + 1 && Math.min(a.y, b.y) < n.y1 - 1
const overlaps = (m: Box, n: Box) => m.x1 > n.x0 && m.x0 < n.x1 && m.y1 > n.y0 && m.y0 < n.y1

test('the ELK family covers the fixtures it should', () => {
  const types = new Set(CASES.map(([, t]) => t))
  expect([...types].sort()).toEqual(['Entities', 'Graph', 'GraphDelta', 'StateMachine', 'Swimlane', 'Tree'])
  expect(CASES.length).toBeGreaterThanOrEqual(12)
})

test.each(CASES)('%s (%s): every edge section is orthogonal and ends at a node border', async (_f, _t, root) => {
  const l = await layoutOf(root)
  expect(l.nodes.length).toBeGreaterThan(0)
  const boxes = l.nodes.map((n) => box(n, 1))
  for (const e of l.edges) {
    for (const s of e.sections) {
      const pts = [s.start, ...s.bends, s.end]
      for (let i = 1; i < pts.length; i++) {
        const a = pts[i - 1]!
        const b = pts[i]!
        expect(Math.abs(a.x - b.x) < 0.5 || Math.abs(a.y - b.y) < 0.5, `edge ${e.index} segment ${i} is diagonal`).toBe(true)
      }
      expect(boxes.some((n) => inside(s.start, n)), `edge ${e.index} starts off every node`).toBe(true)
      expect(boxes.some((n) => inside(s.end, n)), `edge ${e.index} ends off every node`).toBe(true)
    }
  }
})

test.each(CASES)('%s (%s): no edge segment runs through a node it does not end at (§5.4 #5)', async (_f, _t, root) => {
  const l = await layoutOf(root)
  for (const e of l.edges) {
    for (const s of e.sections) {
      const ends = l.nodes.filter((n) => inside(s.start, box(n, 1)) || inside(s.end, box(n, 1))).map((n) => n.id)
      const pts = [s.start, ...s.bends, s.end]
      for (let i = 1; i < pts.length; i++) {
        for (const n of l.nodes) {
          if (ends.includes(n.id)) continue
          expect(crosses(pts[i - 1]!, pts[i]!, box(n)), `edge ${e.index} segment ${i} crosses node ${n.id}`).toBe(false)
        }
      }
    }
  }
})

test.each(CASES)('%s (%s): every edge-label mask is off its stroke (§5.4 #2) and over no node box (§5.4 #5)', async (_f, _t, root) => {
  const l = await layoutOf(root)
  const text: Record<number, string> = {}
  const collect = (n: ElkNode) => {
    for (const e of n.edges ?? []) text[Number(e.id.slice(2))] = e.labels?.[0]?.text ?? ''
    for (const c of n.children ?? []) collect(c)
  }
  collect(root)
  for (const e of l.edges) {
    const shown = truncate(text[e.index] ?? '')
    if (!shown) continue
    const { at, along, side } = edgeLabelAt(e)
    const mask = box(maskRect(shown, at, along, side))
    for (const s of e.sections) {
      const pts = [s.start, ...s.bends, s.end]
      for (let i = 1; i < pts.length; i++) expect(crosses(pts[i - 1]!, pts[i]!, mask), `label "${shown}" sits on edge ${e.index} segment ${i}`).toBe(false)
    }
    for (const n of l.nodes) expect(overlaps(mask, box(n)), `label "${shown}" of edge ${e.index} covers node ${n.id}`).toBe(false)
  }
})
