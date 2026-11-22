// 208 §8.6: what the canvas draws — LOD buckets, folds, highlight dimming, edge handles.
import { expect, test } from 'vitest'
import type { GraphNode } from '@/rpc/bindings/method.agent.output'
import { canvasElements, FOLD_MIN, folds, initialViewport, lodOf } from './elements'
import { GRAPH_FIXTURE, GRAPH_FIXTURE_ATTENTION as ATT, GRAPH_FIXTURE_NOW as T0 } from './fixture'
import { fromSnapshot, type GraphState } from './reducer'
import { TTL_DEFAULTS, visibleGraph } from './ttl'

const S = fromSnapshot(GRAPH_FIXTURE)

test('lodOf: ≥ 0.75 full, ≥ 0.4 compact, else dot', () => {
  expect([lodOf(2), lodOf(0.75), lodOf(0.74), lodOf(0.4), lodOf(0.39), lodOf(0.1)]).toEqual(['full', 'full', 'compact', 'compact', 'dot', 'dot'])
})

test('a parent with ≥ 8 unpinned visible descendants folds them into one chip; pinned or expanded ones do not', () => {
  const r2 = S.nodes.get('r2') as GraphNode
  const kids: GraphNode[] = Array.from({ length: FOLD_MIN }, (_, i) => ({ ...r2, id: `f${i}`, kind: 'child', parent: 'r2', root: 'r2', name: `f${i}`, last_event_ms: T0, started_ms: T0 + i }))
  const state: GraphState = { ...S, nodes: new Map([...S.nodes, ...kids.map((k) => [k.id, k] as const)]) }
  const view = visibleGraph(state, ATT, TTL_DEFAULTS, T0)
  const f = folds(view, new Set())
  expect(f.get('r2')?.length).toBeGreaterThanOrEqual(FOLD_MIN)
  expect(f.has('r1')).toBe(false)
  expect(folds(view, new Set(['r2'])).has('r2')).toBe(false)
  const els = canvasElements(state, view, null, new Set(), null)
  expect(els.nodes.some((n) => n.id === 'fold:r2' && n.type === 'fold')).toBe(true)
  expect(els.nodes.some((n) => n.id === 'f0')).toBe(false)
  expect(els.edges.some((e) => e.id === 't:fold:r2' && e.source === 'r2')).toBe(true)
})

test('highlight: the path is lit, everything else dimmed; message edges pick arc, side or vertical handles', () => {
  const view = visibleGraph(S, ATT, TTL_DEFAULTS, T0)
  const els = canvasElements(S, view, 'r1.builder.linter', new Set(), null)
  const by = new Map(els.nodes.map((n) => [n.id, n]))
  expect(by.get('r1.builder')).toMatchObject({ highlighted: true, dim: false })
  expect(by.get('r1')).toMatchObject({ highlighted: true, dim: false })
  expect(by.get('r2')).toMatchObject({ highlighted: false, dim: true })
  const tree = els.edges.find((e) => e.id === 't:r1.builder.linter')
  expect(tree).toMatchObject({ onPath: true, dim: false })
  // linter sits below builder (same column band): bottom → top; r1 and r2 are roots on one row with cards between
  // them: an arc over the row, top → top, its arrow toward r2 (r1 wrote once, r2 never).
  const linter = els.edges.find((e) => e.id === 'r1.builder|r1.builder.linter')
  expect(linter).toMatchObject({ type: 'message', count: 3, sourceHandle: 'b', targetHandle: 't' })
  const across = els.edges.find((e) => e.id === 'r1|r2')
  expect(across).toMatchObject({ source: 'r2', target: 'r1', sourceHandle: 'ts', targetHandle: 't', fwd: 0, back: 1 })
  // The arc's room above the top row is part of the drawing's extent.
  expect(els.extent.y).toBeLessThan(0)
  // builder and reviewer share a row with docs between them: an arc; adjacent siblings (claude, codex) talk across the
  // gap, side to side.
  expect(els.pos.get('r1.builder')?.y).toBe(els.pos.get('r1.reviewer')?.y)
  expect(els.edges.find((e) => e.id === 'r1.builder|r1.reviewer')).toMatchObject({ sourceHandle: 'ts', targetHandle: 't' })
  expect(els.edges.find((e) => e.id === 'r1.claude|r1.codex')).toMatchObject({ sourceHandle: 'r', targetHandle: 'l' })
  expect(canvasElements(S, view, null, new Set(), null).nodes.every((n) => !n.dim)).toBe(true)
})

test('opening viewport: a wide forest fits its rows at full detail, left-aligned; a small one is whole and centred', () => {
  const wide = { x: 0, y: -60, w: 4000, h: 300 }
  const v = initialViewport(wide, 1400, 400)
  expect(lodOf(v.zoom)).toBe('full')
  expect(v.x).toBeCloseTo(32)
  // Rows fit the height: top and bottom of the drawing are on screen.
  expect(-60 * v.zoom + v.y).toBeGreaterThanOrEqual(0)
  expect(240 * v.zoom + v.y).toBeLessThanOrEqual(400)
  const small = initialViewport({ x: 0, y: 0, w: 400, h: 200 }, 1400, 400)
  expect(small).toEqual({ x: 500, y: 100, zoom: 1 })
  // Fit (floor 0.1) shows the whole wide forest.
  expect(4000 * initialViewport(wide, 1400, 400, 0.1).zoom).toBeLessThanOrEqual(1400)
  // A short region never opens at dot level.
  expect(initialViewport({ x: 0, y: 0, w: 4000, h: 2000 }, 1400, 300).zoom).toBe(0.4)
})