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
import type { CodeMapChanged, CodeMapProgress, MapEdge } from './map-types'

const ROOT = '/Users/sky/Develop/theuth'
const events: (CodeMapProgress | CodeMapChanged)[] = []
const api = mockMapApi((e) => events.push(e))
const structure = await api.codemap_overview(ROOT)
const unnamed = await api.codemap_element(ROOT, 'request-lifecycle')
// the shared fixtures are the generated map: run the mock's generation under a fake clock first
vi.useFakeTimers()
const job = await api.codemap_generate(ROOT, 'sess', null)
vi.advanceTimersByTime(500 + 700 * (job.total + 1))
vi.useRealTimers()
const index = await api.codemap_overview(ROOT)
const top = await api.codemap_element(ROOT, 'overall-architecture')
const stores = await api.codemap_element(ROOT, 'overall-architecture/cluster:9f3a1c2e')

test('tree builds from MapPerspective.tree: headers first, pre-order, folds hide subtrees, depth ≤ 3', () => {
  const all = buildTreeRows(index, () => true)
  expect(all[0]).toMatchObject({ kind: 'perspective', id: 'overall-architecture', depth: 0, open: true })
  const names = all.map((r) => r.name)
  // pre-order: Content stores, then its five children, then codeindex's nine
  const stores = names.indexOf('Content stores')
  expect(names.slice(stores + 1, stores + 6)).toEqual(['Document authority', 'Knowledge bundle', 'Artifacts', 'Search envelope', 'Code index'])
  expect(names[stores + 6]).toBe('Walk')
  expect(Math.max(...all.map((r) => r.depth))).toBe(3)
  // a folded Content stores hides its subtree but not its siblings
  const folded = buildTreeRows(index, (id) => id !== 'overall-architecture/cluster:9f3a1c2e')
  expect(folded.map((r) => r.name)).not.toContain('Code index')
  expect(folded.map((r) => r.name)).toContain('World access')
  expect(folded.find((r) => r.name === 'Content stores')).toMatchObject({ foldable: true, open: false, count: 5 })
  // perspective rows carry element counts, stale counts and the imported badge
  const imported = all.find((r) => r.id === 'imported:.omm')!
  expect(imported).toMatchObject({ kind: 'perspective', origin: 'Imported', count: 3 })
  expect(all.find((r) => r.id === 'overall-architecture')!.staleCount).toBe(1)
  expect(all.find((r) => r.name === 'Git facts')).toMatchObject({ stale: true, leaf: true })
})

test('default open set: first perspective plus the selection and its ancestors', () => {
  const open = defaultOpen(index, 'overall-architecture/unit:crates/codeindex/src/git.rs')
  expect([...open]).toEqual(expect.arrayContaining(['overall-architecture', 'overall-architecture/cluster:9f3a1c2e', 'overall-architecture/unit:crates/codeindex']))
  expect(open.has('request-lifecycle')).toBe(false)
  expect(parentsOf(index).get('overall-architecture/cluster:9f3a1c2e')).toBe('overall-architecture')
})

test('keyboard table: ↑/↓ move, → unfolds then moves, ← folds then climbs, Enter drills, Esc up', () => {
  const rows = buildTreeRows(index, (id) => id === 'overall-architecture')
  const stores = rows.findIndex((r) => r.name === 'Content stores')
  expect(treeKey('ArrowDown', rows, stores)).toEqual({ type: 'move', to: stores + 1 })
  expect(treeKey('ArrowUp', rows, 0)).toBeNull()
  expect(treeKey('ArrowRight', rows, stores)).toEqual({ type: 'open', id: 'overall-architecture/cluster:9f3a1c2e' })
  expect(treeKey('ArrowLeft', rows, stores)).toEqual({ type: 'move', to: 0 })
  expect(treeKey('ArrowLeft', rows, 0)).toEqual({ type: 'close', id: 'overall-architecture' })
  expect(treeKey('Enter', rows, stores)).toEqual({ type: 'drill', id: 'overall-architecture/cluster:9f3a1c2e' })
  expect(treeKey('Escape', rows, stores)).toEqual({ type: 'up' })
  expect(treeKey('x', rows, stores)).toBeNull()
  const open = buildTreeRows(index, () => true)
  const at = open.findIndex((r) => r.name === 'Content stores')
  expect(treeKey('ArrowRight', open, at)).toEqual({ type: 'move', to: at + 1 })
  expect(treeKey('ArrowLeft', open, at + 1)).toEqual({ type: 'move', to: at })
})

test('activity marks: read from trail targets, edited from the edit list, by path prefix up the tree', () => {
  const marks = marksFor(index, ['crates/codeindex/src/tools.rs#L188', 'a..b', null], ['crates/document/src/anchor.rs'])
  expect(marks.get('overall-architecture/unit:crates/codeindex/src/tools.rs')).toEqual(['read'])
  expect(marks.get('overall-architecture/unit:crates/codeindex')).toEqual(['read'])
  expect(marks.get('overall-architecture/unit:crates/document')).toEqual(['edited'])
  expect(marks.has('overall-architecture/unit:crates/rpc')).toBe(false)
  // the cluster above both: read through codeindex, edited through document; the perspective header never
  expect([...marks.get('overall-architecture/cluster:9f3a1c2e')!].sort()).toEqual(['edited', 'read'])
  expect(marks.has('overall-architecture')).toBe(false)
  const rows = buildTreeRows(index, () => true, marks)
  expect(rows.find((r) => r.name === 'code_* tools')!.marks).toEqual(['read'])
  expect(marksFor(index, [], []).size).toBe(0)
})

test('search: a name hit and a path hit each light the element chain; the mock answers through localSearch', async () => {
  const byName = localSearch(index, 'git facts')
  expect(byName[0]).toMatchObject({ element: 'overall-architecture/unit:crates/codeindex/src/git.rs', chain: ['overall-architecture', 'overall-architecture/cluster:9f3a1c2e', 'overall-architecture/unit:crates/codeindex'] })
  const byPath = localSearch(index, 'crates/codeindex/src/rank.rs')
  expect(byPath[0]?.name).toBe('Rank')
  const chain = chainOf(byPath)
  expect(chain.has('overall-architecture/cluster:9f3a1c2e')).toBe(true)
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
      { Manifest: { from: 'crates/a/Cargo.toml', to: 'crates/b', kind: 'cargo-path' } },
      { Manifest: { from: 'crates/c/Cargo.toml', to: 'crates/b', kind: 'cargo-path' } },
      { Imports: { count: 37, sample: 'crates/rpc/src/api.rs#L1079' } },
      { Refs: { count: 1, sample: 'x.rs#L1', confidence: 0.9 } },
      { Call: { site: 'y.rs#L2', from: 'f', to: 'g' } },
    ],
  }
  expect(evidenceCount(e.evidence)).toBe('2 Cargo deps · 37 imports · 1 ref · 1 call site')
  expect(evidenceSummary(e)).toContain('sample crates/rpc/src/api.rs#L1079')
  const inf: MapEdge = { from: 'a', to: 'b', label: 'WebSocket', backed: false, evidence: [{ Inferred: { reason: 'no manifest edge' } }] }
  expect(evidenceCount(inf.evidence)).toBe('')
  expect(evidenceSummary(inf)).toBe('inferred: no manifest edge')
  expect(evidenceCount([{ Bindings: { generator: 'ts-rs', output_dir: 'bindings/' } }, { Process: { site: 's', verb: 'spawn' } }])).toBe('generated bindings · spawn')
})

test('elementGraph: nodes re-labelled from children (name · path), inferred edges dashed with why · count labels, stale hatched, thumbnails on groups', () => {
  const g = elementGraph(top, 'TB')
  expect(g.graph.direction).toBe('TB')
  expect(g.graph.nodes).toHaveLength(11)
  const storesNode = g.graph.nodes.find((n) => n.id === 'cluster:9f3a1c2e')!
  expect(storesNode).toMatchObject({ label: 'Content stores', sub: 'crates/knowledge', ref: 'crates/knowledge' })
  expect(g.thumbs.has('cluster:9f3a1c2e')).toBe(true)
  expect(g.thumbs.has('unit:crates/rpc')).toBe(false)
  const ws = g.graph.edges.find((e) => e.from === 'cluster:1a2b3c4d')!
  expect(ws.style).toBe('dashed')
  expect(ws.label).toBe('WebSocket JSON-RPC')
  const rpc = g.graph.edges.find((e) => e.from === 'unit:crates/rpc' && e.to === 'cluster:9f3a1c2e')!
  expect(rpc).toMatchObject({ style: 'solid', label: 'lens, doc, knowledge · 5 Cargo deps' })
  expect(elementStats(top)).toBe('11 elements · 14 of 15 edges')
  const labels = new Map(g.graph.nodes.map((n) => [n.id, n.label]))
  expect(footnote(g, labels)).toBe('dashed = inferred (Clients → RPC facade)')
  const ci = elementGraph(stores, 'LR')
  expect(ci.hatch).toEqual([])
  const deep = elementGraph({ ...stores, children: stores.children.map((c) => ({ ...c, stale: c.name === 'Code index' })) }, 'LR')
  expect(deep.hatch).toEqual(['unit:crates/codeindex'])
  expect(abbrevPath(['a/b', 'c/d', 'e', 'f'])).toBe('a/b · c/d +2')
  expect(abbrevPath(['a'])).toBe('a')
})

test('snapshot: a parseSpec-valid Graph fence, ≤ 12 nodes, ref on every node, dashed kept', () => {
  const spec = mapGraphSpec(top) as { elements: { g: { props: { nodes: { id: string; ref: string; label: string }[]; edges: { style?: string }[] } } } }
  expect(parseSpec(JSON.stringify(spec))).toMatchObject({ repaired: false, spec: { root: 'g' } })
  const g = spec.elements.g.props
  expect(g.nodes.length).toBeLessThanOrEqual(12)
  for (const n of g.nodes) expect(n.ref, n.id).toBeTruthy()
  expect(g.edges.some((e) => e.style === 'dashed')).toBe(true)
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

test('lines: progress (stage · n of m · $ · calls), provenance (written by · HEAD · ago · $ · calls), imported, structure, list title', () => {
  expect(progressLine({ stage: 'top', done: 5, total: 9, calls: 7, cost_nanos: 310_000_000, currency: 'USD', error: null })).toBe('Mapping perspectives · 5 of 9 · $0.31 · 7 calls')
  expect(progressLine({ stage: 'failed', done: 1, total: 9, calls: 0, cost_nanos: 0, currency: 'USD', error: 'session runs on an external harness' })).toBe('Generation failed · 1 of 9 · session runs on an external harness')
  expect(money(4_000_000)).toBe('<0.01'.replace('<0.01', '$<0.01'))
  expect(money(0)).toBe('$0.00')
  const now = Date.now()
  expect(provenanceLine('Generated', { ...stores.provenance!, generated_ms: now - 3 * 60_000, git_head: '5cddad8e9f', cost_nanos: 40_000_000, call_ids: ['a', 'b'] }, now)).toBe('written by claude-fable-5-1 · HEAD 5cddad8e · 3 min ago · $0.04 · 2 calls')
  expect(provenanceLine('Imported', null)).toBe('imported · unknown freshness')
  expect(provenanceLine('Structure', null)).toContain('structure view')
  expect(listTitle(top)).toMatch(/^11 elements · 14 of 15 edges · written /)
})

test('mock generation: Structure (one perspective, basenames, no prose) → progress per element → done; CodeMapChanged names what to refetch', async () => {
  expect(structure.meta.state).toBe('Structure')
  expect(structure.perspectives.map((p) => p.id)).toEqual(['overall-architecture'])
  expect(structure.perspectives[0]!.tree.find((r) => r.node === 'cluster:9f3a1c2e')!.name).toBe('knowledge')
  expect(unnamed.origin).toBe('Structure')
  expect(unnamed.fields.description).toBe('')
  expect(job).toMatchObject({ stage: 'done', done: job.total })
  expect(job.total).toBeGreaterThan(10)
  const kinds = events.map((e) => e.type)
  expect(kinds).toContain('CodeMapProgress')
  expect(kinds).toContain('CodeMapChanged')
  expect(events.at(-1)).toMatchObject({ type: 'CodeMapChanged', root: ROOT })
  expect((events.filter((e) => e.type === 'CodeMapProgress') as CodeMapProgress[]).at(-1)!.stage).toBe('done')
  expect(index.meta.state).toBe('Ready')
  expect(index.perspectives.map((p) => p.id)).toEqual(['overall-architecture', 'request-lifecycle', 'imported:.omm'])
  expect(stores.fields.description).toMatch(/^Durable content/)
  expect(stores.also_in).toEqual([])
  expect(await api.codemap_export(ROOT).catch((e: Error) => e.message)).toMatch(/refused/)
})
