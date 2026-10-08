import { expect, test, vi } from 'vitest'
import { parseSpec } from '@/components/chat/surface/parse'
import { listTitle } from '@/components/codemap/MapList'
import {
  abbrevPath,
  buildTreeRows,
  chainOf,
  decodeMapSearch,
  defaultOpen,
  directionFor,
  edgeLabelShown,
  elementGraph,
  elementStats,
  encodeMapSearch,
  evidenceCount,
  evidenceSummary,
  footnote,
  localSearch,
  mapGraphSpec,
  marksFor,
  money,
  parentsOf,
  progressLine,
  provenanceLine,
  thumbLabels,
  thumbTransform,
  treeKey,
} from './map'
import { mockMapApi } from './map-adapter'
import type { CodeMapChanged, CodeMapProgress } from './types'
import type { MapEdge } from '@/rpc/bindings/MapEdge'

const ROOT = '/Users/sky/Develop/theuth'
const STORES = 'overall-architecture/cluster:26b565b0'
const CODEINDEX = 'overall-architecture/unit:crates/codeindex'
const GIT = 'overall-architecture/unit:crates/codeindex/src/git.rs'
const events: (CodeMapProgress | CodeMapChanged)[] = []
const api = mockMapApi((e) => events.push(e))
const structure = await api.codemap_overview(ROOT)
const unnamed = await api.codemap_element(ROOT, 'overall-architecture')
// the shared fixtures are the generated map: replay the mock's generation under a fake clock first
vi.useFakeTimers()
const job = await api.codemap_generate(ROOT, 'sess', null)
vi.advanceTimersByTime(700 * 8)
vi.useRealTimers()
const index = await api.codemap_overview(ROOT)
const top = await api.codemap_element(ROOT, 'overall-architecture')
const stores = await api.codemap_element(ROOT, STORES)
const codeindex = await api.codemap_element(ROOT, CODEINDEX)

test('tree builds from MapPerspective.tree: headers first (the depth-0 row folds into the header), pre-order, folds hide subtrees, depth ≤ 3', () => {
  const all = buildTreeRows(index, () => true)
  expect(all[0]).toMatchObject({ kind: 'perspective', id: 'overall-architecture', depth: 0, open: true, count: 29, staleCount: 1 })
  expect(all.filter((r) => r.id === 'overall-architecture')).toHaveLength(1)
  const names = all.map((r) => r.name)
  const at = names.indexOf('Content stores')
  expect(names.slice(at + 1, at + 6)).toEqual(['Document authority', 'Knowledge bundle', 'Artifacts', 'Search envelope', 'Code index'])
  expect(names[at + 6]).toBe('Walk')
  expect(Math.max(...all.map((r) => r.depth))).toBe(3)
  const folded = buildTreeRows(index, (id) => id !== STORES)
  expect(folded.map((r) => r.name)).not.toContain('Code index')
  expect(folded.map((r) => r.name)).toContain('World access')
  expect(folded.find((r) => r.name === 'Content stores')).toMatchObject({ foldable: true, open: false, count: 5 })
  expect(all.find((r) => r.name === 'Git facts')).toMatchObject({ stale: true, leaf: true })
  expect(all.find((r) => r.id === 'request-lifecycle')).toMatchObject({ kind: 'perspective', count: 6 })
})

test('default open set: first perspective plus the selection and its ancestors', () => {
  const open = defaultOpen(index, GIT)
  expect([...open]).toEqual(expect.arrayContaining(['overall-architecture', STORES, CODEINDEX]))
  expect(open.has('request-lifecycle')).toBe(false)
  expect(parentsOf(index).get(STORES)).toBe('overall-architecture')
  expect(parentsOf(index).get(CODEINDEX)).toBe(STORES)
})

test('keyboard table: ↑/↓ move, → unfolds then moves, ← folds then climbs, Enter drills, Esc up', () => {
  const rows = buildTreeRows(index, (id) => id === 'overall-architecture')
  const at = rows.findIndex((r) => r.name === 'Content stores')
  expect(treeKey('ArrowDown', rows, at)).toEqual({ type: 'move', to: at + 1 })
  expect(treeKey('ArrowUp', rows, 0)).toBeNull()
  expect(treeKey('ArrowRight', rows, at)).toEqual({ type: 'open', id: STORES })
  expect(treeKey('ArrowLeft', rows, at)).toEqual({ type: 'move', to: 0 })
  expect(treeKey('ArrowLeft', rows, 0)).toEqual({ type: 'close', id: 'overall-architecture' })
  expect(treeKey('Enter', rows, at)).toEqual({ type: 'drill', id: STORES })
  expect(treeKey('Escape', rows, at)).toEqual({ type: 'up' })
  expect(treeKey('x', rows, at)).toBeNull()
  const open = buildTreeRows(index, () => true)
  const o = open.findIndex((r) => r.name === 'Content stores')
  expect(treeKey('ArrowRight', open, o)).toEqual({ type: 'move', to: o + 1 })
  expect(treeKey('ArrowLeft', open, o + 1)).toEqual({ type: 'move', to: o })
})

test('activity marks: read from trail targets, edited from the edit list, by path prefix, climbing to ancestors but not the header', () => {
  const marks = marksFor(index, ['crates/codeindex/src/tools.rs#L188', 'a..b', null], ['crates/document/src/anchor.rs'])
  expect(marks.get('overall-architecture/unit:crates/codeindex/src/tools.rs')).toEqual(['read'])
  expect(marks.get(CODEINDEX)).toEqual(['read'])
  expect(marks.get('overall-architecture/unit:crates/document')).toEqual(['edited'])
  expect(marks.has('overall-architecture/unit:crates/rpc')).toBe(false)
  expect([...marks.get(STORES)!].sort()).toEqual(['edited', 'read'])
  expect(marks.has('overall-architecture')).toBe(false)
  const rows = buildTreeRows(index, () => true, marks)
  expect(rows.find((r) => r.name === 'code_* tools')!.marks).toEqual(['read'])
  expect(marksFor(index, [], []).size).toBe(0)
})

test('search: a name hit and a path hit each light the element chain; the mock answers through localSearch', async () => {
  const byName = localSearch(index, 'git facts')
  expect(byName[0]).toMatchObject({ element: GIT, chain: ['overall-architecture', STORES, CODEINDEX] })
  const byPath = localSearch(index, 'crates/codeindex/src/rank.rs')
  expect(byPath[0]?.name).toBe('Rank')
  const chain = chainOf(byPath)
  expect(chain.has(STORES)).toBe(true)
  expect(chain.has('overall-architecture/unit:crates/codeindex/src/rank.rs')).toBe(true)
  expect(chain.has('request-lifecycle')).toBe(false)
  expect(localSearch(index, '  ')).toEqual([])
  expect((await api.codemap_search(ROOT, 'rpc', 5)).length).toBeGreaterThan(0)
})

test('thumbnails: scale fits the box, never above 1; labels hidden below 0.5', () => {
  expect(thumbTransform(400, 200, { w: 148, h: 44 })).toMatchObject({ scale: 0.22 })
  expect(thumbTransform(100, 40, { w: 148, h: 44 }).scale).toBe(1)
  expect(thumbTransform(0, 0, { w: 148, h: 44 }).scale).toBe(1)
  const t = thumbTransform(200, 100, { w: 148, h: 60 })
  expect(t.scale).toBeCloseTo(0.6)
  expect(t.dx).toBeCloseTo((148 - 120) / 2)
  expect(thumbLabels(0.49)).toBe(false)
  expect(thumbLabels(0.5)).toBe(true)
})

test('direction by canvas aspect (O8b): TB in the pane, LR only when wider than tall and ≥ 1000 px', () => {
  expect(directionFor(480, 700)).toBe('TB')
  expect(directionFor(920, 600)).toBe('TB')
  expect(directionFor(999, 600)).toBe('TB')
  expect(directionFor(1000, 600)).toBe('LR')
  expect(directionFor(1200, 1300)).toBe('TB')
})

test('edge labels hide below 70 % zoom except for the selection', () => {
  const e = { from: 'a', to: 'b' }
  expect(edgeLabelShown(0.86, e, null)).toBe(true)
  expect(edgeLabelShown(0.69, e, null)).toBe(false)
  expect(edgeLabelShown(0.3, e, 'b')).toBe(true)
  expect(edgeLabelShown(0.3, e, 'c')).toBe(false)
})

test('evidence renders counts, never the model: Cargo deps, imports, refs, call sites; inferred reason in the summary', () => {
  const e: MapEdge = {
    from: 'a',
    to: 'b',
    label: 'board anchors',
    backed: true,
    evidence: [
      { type: 'manifest', from: 'crates/a/Cargo.toml', to: 'crates/b', kind: 'cargo-path' },
      { type: 'manifest', from: 'crates/c/Cargo.toml', to: 'crates/b', kind: 'cargo-path' },
      { type: 'imports', count: 37, sample: 'crates/rpc/src/api.rs#L1079' },
      { type: 'refs', count: 1, sample: 'x.rs#L1', confidence: 0.9 },
      { type: 'call', site: 'y.rs#L2', from: 'f', to: 'g' },
    ],
  }
  expect(evidenceCount(e.evidence)).toBe('2 Cargo deps · 37 imports · 1 ref · 1 call site')
  expect(evidenceSummary(e)).toContain('sample crates/rpc/src/api.rs#L1079')
  const inf: MapEdge = { from: 'a', to: 'b', label: 'WebSocket', backed: false, evidence: [{ type: 'inferred', reason: 'no manifest edge' }] }
  expect(evidenceCount(inf.evidence)).toBe('')
  expect(evidenceSummary(inf)).toBe('inferred: no manifest edge')
  expect(evidenceCount([{ type: 'bindings', generator: 'ts-rs', output_dir: 'bindings/' }, { type: 'process', site: 's', verb: 'spawn' }])).toBe('generated bindings · spawn')
})

test('elementGraph over the content-stores fixture: nodes from children, why · count labels, thumbnail on the group, inferred edges dashed, stale hatched', () => {
  const g = elementGraph(stores, 'TB')
  expect(g.graph.direction).toBe('TB')
  expect(g.graph.nodes).toHaveLength(5)
  expect(g.graph.nodes.find((n) => n.id === 'unit:crates/codeindex')).toMatchObject({ label: 'Code index', sub: 'crates/codeindex', ref: 'crates/codeindex' })
  expect(g.thumbs.has('unit:crates/codeindex')).toBe(true)
  expect(g.thumbs.has('unit:crates/document')).toBe(false)
  expect(g.graph.edges).toHaveLength(6)
  expect(g.graph.edges.find((e) => e.from === 'unit:crates/knowledge' && e.to === 'unit:crates/document')).toMatchObject({ style: 'solid', label: 'concepts are block documents · 1 Cargo dep' })
  expect(elementStats(stores)).toBe('5 elements · 6 edges')
  expect(g.hatch).toEqual([])
  // one edge turned inferred, one child stale
  const edited = { ...stores, edges: stores.edges.map((e, i) => (i === 0 ? { ...e, backed: false, evidence: [{ type: 'inferred' as const, reason: 'claimed' }] } : e)), children: stores.children.map((c) => (c.name === 'Code index' ? { ...c, stale: true } : c)) }
  const d = elementGraph(edited, 'LR')
  expect(d.graph.edges[0]).toMatchObject({ style: 'dashed', label: 'concepts are block documents' })
  expect(d.hatch).toEqual(['unit:crates/codeindex'])
  expect(elementStats(edited)).toBe('5 elements · 5 of 6 edges')
  const labels = new Map(d.graph.nodes.map((n) => [n.id, n.label]))
  expect(footnote(d, labels)).toBe('dashed = inferred (Knowledge bundle → Document authority)')
  expect(elementGraph(codeindex, 'TB').hatch).toEqual(['unit:crates/codeindex/src/git.rs'])
  // the perspective's own element (ledger.json): 11 children, two with thumbnails, 16 edges of which one is inferred (dashed)
  const t = elementGraph(top, 'TB')
  expect(t.graph.nodes).toHaveLength(11)
  expect(t.graph.edges).toHaveLength(16)
  expect(t.graph.edges.filter((e) => e.style === 'dashed').map((e) => `${e.from}>${e.to}`)).toEqual(['cluster:df1efa65>unit:crates/rpc'])
  expect([...t.thumbs.keys()].sort()).toEqual(['cluster:040101c3', 'cluster:26b565b0'])
  expect(elementStats(top)).toBe('11 elements · 15 of 16 edges')
  // a synthesised (structure) element: children drawn, no edges
  expect(elementGraph(unnamed, 'TB').graph.nodes).toHaveLength(13)
  expect(elementGraph(unnamed, 'TB').graph.edges).toHaveLength(0)
  expect(abbrevPath(['a/b', 'c/d', 'e', 'f'])).toBe('a/b · c/d +2')
  expect(abbrevPath(['a'])).toBe('a')
})

test('snapshot: a parseSpec-valid Graph fence, ≤ 12 nodes, ref on every node', () => {
  const spec = mapGraphSpec(codeindex) as { elements: { g: { props: { nodes: { id: string; ref: string; label: string }[]; edges: { style?: string }[] } } } }
  expect(parseSpec(JSON.stringify(spec))).toMatchObject({ repaired: false, spec: { root: 'g' } })
  const g = spec.elements.g.props
  expect(g.nodes).toHaveLength(9)
  for (const n of g.nodes) expect(n.ref, n.id).toBeTruthy()
  expect(g.edges.length).toBe(16)
})

test('route state: ?tab=map&el=<id> round-trips, defaults omitted, junk → Map with no element', () => {
  expect(decodeMapSearch({})).toEqual({ tab: 'map', el: null })
  expect(decodeMapSearch({ tab: 'fit', el: 'overall-architecture/unit:crates/rpc' })).toEqual({ tab: 'fit', el: 'overall-architecture/unit:crates/rpc' })
  expect(decodeMapSearch({ tab: 'nope', el: 7 })).toEqual({ tab: 'map', el: null })
  expect(encodeMapSearch({ tab: 'map', el: null })).toEqual({})
  expect(encodeMapSearch({ tab: 'map', el: 'x' })).toEqual({ el: 'x' })
  expect(encodeMapSearch({ tab: 'change', el: null })).toEqual({ tab: 'change' })
  const s = { tab: 'here' as const, el: 'overall-architecture' }
  expect(decodeMapSearch(encodeMapSearch(s))).toEqual(s)
})

test('lines: progress (stage · n of m · [$] · calls; no $0.00 in v0), provenance (written by · HEAD · ago · $ · calls), imported, structure, list title', () => {
  expect(progressLine({ stage: 'top', done: 5, total: 9, calls: 7, cost_nanos: 310_000_000, currency: 'USD', error: null })).toBe('Mapping perspectives · 5 of 9 · $0.31 · 7 calls')
  expect(progressLine({ stage: 'top', done: 5, total: 9, calls: 7, cost_nanos: 0, currency: 'USD', error: null })).toBe('Mapping perspectives · 5 of 9 · 7 calls')
  expect(progressLine({ stage: 'failed', done: 1, total: 9, calls: 0, cost_nanos: 0, currency: 'USD', error: 'session runs on an external harness' })).toBe('Generation failed · 1 of 9 · session runs on an external harness')
  expect(money(4_000_000)).toBe('$<0.01')
  const now = Date.now()
  expect(provenanceLine('generated', { ...stores.provenance!, generated_ms: now - 3 * 60_000 }, now)).toBe('written by anthropic/claude-opus-4 · HEAD 5cddad8e · 3 min ago · $0.04 · 2 calls')
  expect(provenanceLine('imported', null)).toBe('imported · unknown freshness')
  expect(provenanceLine('structure', null)).toContain('structure view')
  expect(listTitle(stores)).toMatch(/^5 elements · 6 edges · written /)
  expect(listTitle(top)).toMatch(/^11 elements · 15 of 16 edges · written /)
  expect(listTitle(unnamed)).toBe('13 elements · structure view')
})

test('fixture mock: structure overview (basenames, no prose) → map-progress.json replay → generated overview and fixture elements', async () => {
  expect(structure.meta.state).toBe('structure')
  expect(structure.perspectives.map((p) => p.id)).toEqual(['overall-architecture'])
  expect(structure.perspectives[0]!.tree.find((r) => r.depth === 1)!.name).toBe('desktop')
  expect(unnamed.origin).toBe('structure')
  expect(unnamed.fields.description).toBe('')
  expect(unnamed.crumbs).toEqual([['overall-architecture', 'Overall architecture']])
  expect(job).toMatchObject({ stage: 'done', total: 61 })
  const kinds = events.map((e) => e.type)
  expect(kinds.filter((k) => k === 'CodeMapProgress')).toHaveLength(6)
  expect(kinds).toContain('CodeMapChanged')
  expect(events.every((e) => e.root === ROOT)).toBe(true)
  expect(index.meta.state).toBe('ready')
  expect(index.perspectives.map((p) => p.id)).toEqual(['overall-architecture', 'request-lifecycle'])
  expect(stores.fields.description).toMatch(/^Durable content/)
  expect(stores.crumbs).toEqual([
    ['overall-architecture', 'Overall architecture'],
    [STORES, 'Content stores'],
  ])
  expect(top.children.map((c) => c.name)).toContain('Content stores')
  expect(top.children.find((c) => c.name === 'Content stores')!.thumbnail?.nodes).toHaveLength(5)
  expect(await api.codemap_export(ROOT).catch((e: Error) => e.message)).toMatch(/refused/)
})
