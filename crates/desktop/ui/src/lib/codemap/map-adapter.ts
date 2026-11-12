/**
 * 147 §10.2 — the six map methods behind one interface. `engineMapApi` forwards to the engine `Api` (flat names,
 * positional params); a method the connected engine lacks rejects with a one-line reason so the detail pane can disable
 * its action with that text. `mockMapApi` serves the contract fixtures (`__fixtures__/map-*.json`, owned by the contract
 * wave): the structure overview first, `codemap_generate` replays `map-progress.json` through `emit` (the dev fixture
 * wires `emit` to `dispatch`) and flips to the generated overview; elements without a fixture are synthesised from the
 * overview's tree (names, paths, children, thumbnails — no edges, no prose), which is exactly what the engine's
 * structure view carries.
 */
import elementCodeindex from '@/components/codemap/__fixtures__/map-element-codeindex.json'
import elementStores from '@/components/codemap/__fixtures__/map-element-content-stores.json'
import elementTop from '@/components/codemap/__fixtures__/map-element-overall-architecture.json'
import overviewStructure from '@/components/codemap/__fixtures__/map-overview-structure.json'
import overviewReady from '@/components/codemap/__fixtures__/map-overview.json'
import progressScript from '@/components/codemap/__fixtures__/map-progress.json'
import type { Api } from '@/rpc/bindings/client'
import type { GraphProps } from "@/rpc/bindings/method.codeindex.output"
import type { MapChild } from "@/rpc/bindings/method.codeindex.output"
import type { MapElement } from "@/rpc/bindings/method.codeindex.output"
import type { MapExport } from "@/rpc/bindings/method.codeindex.output"
import type { MapHit } from "@/rpc/bindings/method.codeindex.output"
import type { MapIndex } from "@/rpc/bindings/method.codeindex.output"
import type { MapJob } from "@/rpc/bindings/method.codeindex.output"
import type { MapTreeRow } from "@/rpc/bindings/method.codeindex.output"
import { localSearch } from './map'
import type { CodeMapChanged, CodeMapProgress } from './types'

export interface MapApi {
  codemap_overview(root: string): Promise<MapIndex>
  codemap_element(root: string, element: string): Promise<MapElement>
  codemap_generate(root: string, session: string, element: string | null): Promise<MapJob>
  codemap_cancel(root: string): Promise<MapJob>
  codemap_search(root: string, query: string, limit: number | null): Promise<MapHit[]>
  codemap_export(root: string): Promise<MapExport>
}
export const MAP_METHODS = ['codemap_overview', 'codemap_element', 'codemap_generate', 'codemap_cancel', 'codemap_search', 'codemap_export'] as const satisfies readonly (keyof MapApi)[]

/** Which of the six the engine offers (an older engine lacks all; a partial contract some). */
export function mapMethodsOf(api: unknown): Set<keyof MapApi> {
  const out = new Set<keyof MapApi>()
  if (api && typeof api === 'object') for (const m of MAP_METHODS) if (typeof (api as Record<string, unknown>)[m] === 'function') out.add(m)
  return out
}

/** The engine adapter: each method forwards when present, else rejects with the reason the UI shows in a tooltip. */
export function engineMapApi(api: Api | null): MapApi {
  const has = mapMethodsOf(api)
  const call = <K extends keyof MapApi>(m: K): MapApi[K] =>
    ((...args: unknown[]) =>
      api === null
        ? Promise.reject(new Error('not connected'))
        : has.has(m)
          ? (api as unknown as Record<K, (...a: unknown[]) => Promise<unknown>>)[m](...args)
          : Promise.reject(new Error(`${m} is not available on this engine`))) as unknown as MapApi[K]
  return {
    codemap_overview: call('codemap_overview'),
    codemap_element: call('codemap_element'),
    codemap_generate: call('codemap_generate'),
    codemap_cancel: call('codemap_cancel'),
    codemap_search: call('codemap_search'),
    codemap_export: call('codemap_export'),
  }
}

// ── the fixture mock ───────────────────────────────────────────────────────────────────────────────────────────────

const STRUCTURE = overviewStructure as unknown as MapIndex
const READY = overviewReady as unknown as MapIndex
const ELEMENTS: Record<string, MapElement> = {}
for (const e of [elementTop, elementStores, elementCodeindex] as unknown as MapElement[]) ELEMENTS[e.id] = e
const SCRIPT = progressScript as unknown as (CodeMapProgress | CodeMapChanged)[]
/** Spacing between replayed events (the engine gates `CodeMapProgress` at ≥ 250 ms; the replay reads better slower). */
const STEP_MS = 700

const clone = <T,>(v: T): T => structuredClone(v)

/** A diagram made of tree rows (the structure view's shape: name · path, no edges). */
function graphOfRows(rows: readonly MapTreeRow[]): GraphProps {
  return { nodes: rows.map((r) => ({ id: r.node, label: r.name, sub: r.path, ref: r.path, kind: r.leaf ? 'box' : 'rounded' })), edges: [] } as unknown as GraphProps
}

class FixtureMap {
  generated = false
  job: MapJob | null = null
  seq = 100
  timers: ReturnType<typeof setTimeout>[] = []
  constructor(readonly emit: (e: CodeMapProgress | CodeMapChanged) => void) {}

  overview(root: string): MapIndex {
    const idx = clone(this.generated ? READY : STRUCTURE)
    idx.meta.root = root
    idx.meta.seq = ++this.seq
    idx.meta.job = this.job
    if (this.job && !['done', 'failed', 'cancelled'].includes(this.job.stage)) idx.meta.state = 'generating'
    return idx
  }

  /** The element: its fixture when one exists and the map is generated, else the tree's structure view of it. */
  element(root: string, id: string): MapElement {
    const fixture = this.generated ? ELEMENTS[id] : undefined
    if (fixture) {
      const el = clone(fixture)
      el.meta = { ...el.meta, root, seq: ++this.seq, job: this.job }
      return el
    }
    const idx = this.generated ? READY : STRUCTURE
    const pid = id.split('/')[0]!
    const p = idx.perspectives.find((x) => x.id === pid)
    const row = p?.tree.find((r) => r.id === id)
    if (!p || !row) throw new Error(`no element ${id}`)
    const parentOf = (r: MapTreeRow) => (r.depth === 0 ? null : (r.parent ?? p.id))
    const kids = p.tree.filter((r) => r.depth > 0 && parentOf(r) === id)
    const crumbs: [string, string][] = []
    for (let cur: MapTreeRow | undefined = row; cur; cur = p.tree.find((r) => r.id === parentOf(cur!))) crumbs.unshift([cur.id, cur.name])
    const children: MapChild[] = kids.map((k) => {
      const grand = p.tree.filter((r) => r.depth > 0 && parentOf(r) === k.id)
      return { id: k.id, node: k.node, name: k.name, path: k.path, kind: k.kind, files: 0, lines: 0, share: 0, leaf: k.leaf, stale: k.stale, thumbnail: grand.length ? graphOfRows(grand) : null }
    })
    return {
      meta: { ...clone(idx.meta), root, seq: ++this.seq, job: this.job },
      id,
      node: row.node,
      perspective: p.id,
      parent: parentOf(row),
      crumbs,
      name: row.name,
      path: row.path,
      kind: row.kind,
      members: kids.map((k) => k.node),
      anchors: row.path ? [{ path: row.path, line_start: null, line_end: null, exists: true }] : kids.filter((k) => k.path).map((k) => ({ path: k.path!, line_start: null, line_end: null, exists: true })),
      fields: { description: '', context: null, constraint: null, concern: null, todo: null, note: null },
      diagram: kids.length ? graphOfRows(kids) : null,
      mermaid: null,
      edges: [],
      children,
      also_in: idx.perspectives.filter((x) => x.id !== p.id && x.tree.some((r) => r.node === row.node)).map((x) => `${x.id}/${row.node}`),
      stale: row.stale,
      origin: 'structure',
      provenance: null,
      listing: kids.length ? null : { files: row.path ? [[row.path, 0, 0]] : [], symbols: [], more: 0 },
    }
  }

  /** Replays `map-progress.json` (root/session rewritten), then the generated overview answers. */
  generate(root: string, session: string): MapJob {
    if (this.job && !['done', 'failed', 'cancelled'].includes(this.job.stage)) return this.job
    const first = SCRIPT.find((e): e is CodeMapProgress => e.type === 'CodeMapProgress')!
    const job: MapJob = { id: first.job, session, started_ms: Date.now(), stage: first.stage, done: first.done, total: first.total, calls: 0, cost_nanos: 0, currency: first.currency, error: null }
    this.job = job
    SCRIPT.forEach((e, i) => {
      this.timers.push(
        setTimeout(() => {
          const ev = { ...clone(e), root, seq: ++this.seq }
          if (ev.type === 'CodeMapProgress') {
            ev.session = session
            Object.assign(job, { stage: ev.stage, done: ev.done, total: ev.total, calls: ev.calls, cost_nanos: ev.cost_nanos, error: ev.error })
            if (ev.stage === 'done') this.generated = true
          } else this.generated = true
          this.emit(ev)
        }, STEP_MS * (i + 1)),
      )
    })
    return job
  }

  cancel(): MapJob {
    for (const t of this.timers) clearTimeout(t)
    this.timers = []
    if (this.job) this.job.stage = 'cancelled'
    return this.job ?? { id: 'none', session: '', started_ms: 0, stage: 'cancelled', done: 0, total: 0, calls: 0, cost_nanos: 0, currency: 'USD', error: null }
  }
}

/** One mock per page; `emit` receives the replayed events (the dev fixture dispatches them into the store). */
export function mockMapApi(emit: (e: CodeMapProgress | CodeMapChanged) => void): MapApi & { mock: FixtureMap } {
  const mock = new FixtureMap(emit)
  return {
    mock,
    async codemap_overview(root) {
      return mock.overview(root)
    },
    async codemap_element(root, element) {
      return mock.element(root, element)
    },
    async codemap_generate(root, session) {
      return mock.generate(root, session)
    },
    async codemap_cancel() {
      return mock.cancel()
    },
    async codemap_search(root, query, limit) {
      return localSearch(mock.overview(root), query, limit ?? 20)
    },
    async codemap_export() {
      throw new Error('<root>/.omm/ exists and was not written by theuth (no generator: key) — export refused')
    },
  }
}
