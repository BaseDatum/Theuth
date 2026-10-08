/**
 * 147 §11 — pure functions behind the Map tab: the sidebar tree from `MapPerspective.tree`, activity marks (§11.8), the
 * search chain (§11.3), the element's drawable graph with evidence-rendered edge labels (O8b), thumbnail scaling with the
 * W6 label rule, direction by canvas aspect (O8b), route state (§11.5), progress/provenance lines (§11.6, §8), the board
 * fence (§13.3). No React, no DOM; `map.test.ts` covers the tables.
 */
import type { GraphData } from '@/lib/graph-layout'
import type { GraphProps } from '@/rpc/bindings/GraphProps'
import { anchorOf } from './labels'
import type { CodeMapProgress, MapChild, MapEdge, MapElement, MapEvidence, MapHit, MapIndex, MapJob, MapOrigin, MapProvenance, MapTreeRow } from './map-types'

export type Mark = 'read' | 'edited'
export type Tab = 'map' | 'here' | 'fit' | 'change'
export const TABS: readonly Tab[] = ['map', 'here', 'fit', 'change']

// ── §11.2 tree ──────────────────────────────────────────────────────────────────────────────────────────────────────

export type TreeRow = {
  /** Element id; a perspective header's id is the perspective id (its root element). */
  id: string
  kind: 'perspective' | 'element'
  /** 0 = perspective header, 1.. = tree depth (`MapTreeRow.depth`, a depth-0 tree row is the header itself). */
  depth: number
  name: string
  path: string | null
  /** Perspective: elements; element: children. */
  count: number
  leaf: boolean
  stale: boolean
  /** Perspective: elements below it that are stale. */
  staleCount: number
  origin: MapOrigin
  marks: Mark[]
  /** Has rows under it in the tree. */
  foldable: boolean
  open: boolean
  bundle: boolean
}

/** Perspective id → its rows; the perspective's own depth-0 row (when the engine includes it) is folded into the header. */
function rowsOf(p: MapIndex['perspectives'][number]): MapTreeRow[] {
  return p.tree.filter((r) => !(r.depth === 0 || r.id === p.id))
}

/** Element id → parent id (perspective id for top-level rows) over the whole index. */
export function parentsOf(index: MapIndex): Map<string, string> {
  const out = new Map<string, string>()
  for (const p of index.perspectives) for (const r of rowsOf(p)) out.set(r.id, r.parent && r.parent !== r.id ? r.parent : p.id)
  return out
}

/** Ancestors of `id` nearest-first (parent … perspective); unknown ids yield what the id's `perspective/` prefix says. */
export function ancestorsOf(id: string, parents: ReadonlyMap<string, string>): string[] {
  const out: string[] = []
  let cur: string | undefined = id
  const seen = new Set<string>()
  while (cur !== undefined && !seen.has(cur)) {
    seen.add(cur)
    const p: string | undefined = parents.get(cur)
    if (p === undefined) break
    out.push(p)
    cur = p
  }
  if (out.length === 0 && id.includes('/')) out.push(id.slice(0, id.indexOf('/')))
  return out
}

/** The rows open by default: the first perspective (or the selected element's) and every ancestor of the selection. */
export function defaultOpen(index: MapIndex, selected: string | null): Set<string> {
  const parents = parentsOf(index)
  const open = new Set<string>()
  const first = index.perspectives[0]?.id
  if (first) open.add(first)
  if (selected) {
    open.add(selected)
    for (const a of ancestorsOf(selected, parents)) open.add(a)
  }
  return open
}

/**
 * The visible tree: perspective headers, then their elements pre-order to depth 3, rows under a closed one hidden.
 * `isOpen(id)` decides folds; `marks` (§11.8) are merged with the engine's `MapTreeRow.marks`.
 */
export function buildTreeRows(index: MapIndex, isOpen: (id: string) => boolean, marks?: ReadonlyMap<string, readonly Mark[]>): TreeRow[] {
  const out: TreeRow[] = []
  for (const p of index.perspectives) {
    const rows = rowsOf(p)
    const kids = new Map<string, number>()
    for (const r of rows) kids.set(r.parent ?? p.id, (kids.get(r.parent ?? p.id) ?? 0) + 1)
    const pOpen = isOpen(p.id)
    out.push({
      id: p.id,
      kind: 'perspective',
      depth: 0,
      name: p.title,
      path: null,
      count: p.elements,
      leaf: rows.length === 0,
      stale: false,
      staleCount: p.stale,
      origin: p.origin,
      marks: [...(marks?.get(p.id) ?? [])],
      foldable: rows.length > 0,
      open: pOpen,
      bundle: false,
    })
    if (!pOpen) continue
    // pre-order with a hidden-ancestor filter: a row shows iff every ancestor up to the header is open
    const hidden = new Set<string>()
    for (const r of rows) {
      const parent = r.parent && r.parent !== p.id ? r.parent : null
      if (parent && (hidden.has(parent) || !isOpen(parent))) {
        hidden.add(r.id)
        continue
      }
      const n = kids.get(r.id) ?? 0
      out.push({
        id: r.id,
        kind: 'element',
        depth: Math.max(1, r.depth),
        name: r.name,
        path: r.path,
        count: n || r.children,
        leaf: r.leaf,
        stale: r.stale,
        staleCount: 0,
        origin: r.origin,
        marks: mergeMarks(r.marks, marks?.get(r.id)),
        foldable: n > 0,
        open: n > 0 && isOpen(r.id),
        bundle: r.node.startsWith('bundle:'),
      })
    }
  }
  return out
}

function mergeMarks(a: readonly string[], b?: readonly Mark[]): Mark[] {
  const set = new Set<Mark>()
  for (const m of a) if (m === 'read' || m === 'edited') set.add(m)
  for (const m of b ?? []) set.add(m)
  return [...set]
}

/** Keyboard table (§11.2): ↑/↓ move, →/← fold, Enter drill, Esc up one level. `null` = not handled. */
export type TreeAction = { type: 'move'; to: number } | { type: 'open'; id: string } | { type: 'close'; id: string } | { type: 'drill'; id: string } | { type: 'up' }
export function treeKey(key: string, rows: readonly TreeRow[], at: number): TreeAction | null {
  const row = rows[at]
  if (!row) return null
  switch (key) {
    case 'ArrowDown':
      return at + 1 < rows.length ? { type: 'move', to: at + 1 } : null
    case 'ArrowUp':
      return at > 0 ? { type: 'move', to: at - 1 } : null
    case 'ArrowRight':
      return row.foldable && !row.open ? { type: 'open', id: row.id } : at + 1 < rows.length && row.open ? { type: 'move', to: at + 1 } : null
    case 'ArrowLeft': {
      if (row.open) return { type: 'close', id: row.id }
      for (let i = at - 1; i >= 0; i--) if (rows[i]!.depth < row.depth) return { type: 'move', to: i }
      return null
    }
    case 'Enter':
      return { type: 'drill', id: row.id }
    case 'Escape':
      return { type: 'up' }
  }
  return null
}

// ── §11.8 activity marks ────────────────────────────────────────────────────────────────────────────────────────────

const filePath = (target: string) => (target.includes('..') ? '' : target.split('#')[0]!)
const under = (dir: string, file: string) => file === dir || file.startsWith(`${dir.endsWith('/') ? dir : `${dir}/`}`)

/**
 * `read` for elements whose `path` contains a `code_*` target (`codemap.trail[].target`), `edited` for a file from
 * `codemap.edited[root]`; prefix match on the tree rows' paths, client side only. A mark climbs to the ancestors (their
 * anchors contain the child's); perspective headers stay unmarked.
 */
export function marksFor(index: MapIndex, trailTargets: readonly (string | null)[], edited: readonly string[]): Map<string, Mark[]> {
  const read = trailTargets.map((t) => (t ? filePath(t) : '')).filter(Boolean)
  const out = new Map<string, Mark[]>()
  if (read.length === 0 && edited.length === 0) return out
  const parents = parentsOf(index)
  const add = (id: string, m: Mark) => {
    const cur = out.get(id) ?? []
    if (!cur.includes(m)) out.set(id, [...cur, m])
  }
  for (const p of index.perspectives) {
    for (const r of rowsOf(p)) {
      if (!r.path) continue
      const marks: Mark[] = []
      if (read.some((f) => under(r.path!, f))) marks.push('read')
      if (edited.some((f) => under(r.path!, f))) marks.push('edited')
      for (const m of marks) for (const id of [r.id, ...ancestorsOf(r.id, parents).filter((a) => a !== p.id)]) add(id, m)
    }
  }
  return out
}

// ── §11.3 search → chain ────────────────────────────────────────────────────────────────────────────────────────────

/** Every element lit by the hits: the hits themselves and their chains (ancestors), so the tree can open to them. */
export function chainOf(hits: readonly MapHit[]): Set<string> {
  const out = new Set<string>()
  for (const h of hits) {
    out.add(h.element)
    for (const c of h.chain) out.add(c)
  }
  return out
}

/** A local `codemap_search` over the tree (the mock adapter and the offline fallback): name substring or path prefix/suffix. */
export function localSearch(index: MapIndex, query: string, limit = 20): MapHit[] {
  const q = query.trim().toLowerCase()
  if (!q) return []
  const parents = parentsOf(index)
  const hits: { hit: MapHit; score: number }[] = []
  for (const p of index.perspectives) {
    for (const r of rowsOf(p)) {
      const name = r.name.toLowerCase()
      const path = (r.path ?? '').toLowerCase()
      // name exact · path exact · name contains · element contains the file (deepest first) · file under the element · path contains
      const score = name === q ? 0 : path === q ? 1 : name.includes(q) ? 2 : path && under(path, q) ? 3 + (1 - path.length / 1000) : path && under(q, path) ? 5 : path.includes(q) ? 6 : -1
      if (score < 0) continue
      hits.push({ hit: { element: r.id, name: r.name, path: r.path, chain: ancestorsOf(r.id, parents).reverse() }, score })
    }
  }
  return hits
    .sort((a, b) => a.score - b.score || a.hit.name.localeCompare(b.hit.name))
    .slice(0, limit)
    .map((h) => h.hit)
}

// ── O8b direction · W6 thumbnails · zoom ───────────────────────────────────────────────────────────────────────────

export const LR_MIN_W = 1000
/** TB in the docked pane; LR only for a canvas wider than tall and ≥ 1 000 px (pop-out). */
export const directionFor = (w: number, h: number): 'LR' | 'TB' => (w > h && w >= LR_MIN_W ? 'LR' : 'TB')

/** The node box of a child that carries a thumbnail (§11.3; pop-out 200×120, §11.9). */
export const thumbBox = (popped: boolean) => (popped ? { w: 200, h: 120 } : { w: 168, h: 104 })
/** Title band above the thumbnail inside its node box. */
export const THUMB_TITLE_H = 36
/** Space kept for the `n inside ›` chip under the thumbnail. */
export const THUMB_CHIP_H = 18

/** Scale and offset that fit a child layout of `w × h` into `box`, centred, never above 1. */
export function thumbTransform(w: number, h: number, box: { w: number; h: number }): { scale: number; dx: number; dy: number } {
  const scale = w > 0 && h > 0 ? Math.min(box.w / w, box.h / h, 1) : 1
  return { scale, dx: (box.w - w * scale) / 2, dy: (box.h - h * scale) / 2 }
}
/** W6: thumbnail labels are drawn only at ≥ 50 % scale — below that boxes and edges only. */
export const thumbLabels = (scale: number): boolean => scale >= 0.5

/** Edge labels hide below ~70 % zoom, except for the selected node's edges (§11.3). */
export const EDGE_LABEL_ZOOM = 0.7
export function edgeLabelShown(zoom: number, edge: { from: string; to: string }, selected: string | null): boolean {
  if (selected !== null && (edge.from === selected || edge.to === selected)) return true
  return zoom >= EDGE_LABEL_ZOOM
}

// ── evidence → rendered counts (§6.6 rule 7: numbers come from evidence, never from the model) ────────────────────

const MANIFEST: Record<string, string> = { 'cargo-path': 'Cargo dep', 'npm-workspace': 'npm dep', pyproject: 'pyproject dep' }
const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`

/** `5 Cargo deps · 37 imports · 12 refs · 2 call sites · bindings · spawn` — one phrase per evidence kind, in that order. */
export function evidenceCount(evidence: readonly MapEvidence[]): string {
  const manifest = new Map<string, number>()
  let imports = 0
  let refs = 0
  let calls = 0
  const other: string[] = []
  for (const ev of evidence) {
    if ('Manifest' in ev) manifest.set(ev.Manifest.kind, (manifest.get(ev.Manifest.kind) ?? 0) + 1)
    else if ('Imports' in ev) imports += ev.Imports.count
    else if ('Refs' in ev) refs += ev.Refs.count
    else if ('Call' in ev) calls += 1
    else if ('Bindings' in ev) other.push('generated bindings')
    else if ('Process' in ev) other.push(ev.Process.verb)
  }
  const parts: string[] = []
  for (const [kind, n] of manifest) parts.push(plural(n, MANIFEST[kind] ?? `${kind} dep`))
  if (imports) parts.push(plural(imports, 'import'))
  if (refs) parts.push(plural(refs, 'ref'))
  if (calls) parts.push(plural(calls, 'call site'))
  parts.push(...new Set(other))
  return parts.join(' · ')
}

/** The tooltip / detail text: counts plus one sample per kind, the inferred reason last. */
export function evidenceSummary(e: MapEdge): string {
  const parts = [evidenceCount(e.evidence)].filter(Boolean)
  for (const ev of e.evidence) {
    if ('Imports' in ev) parts.push(`sample ${ev.Imports.sample}`)
    else if ('Refs' in ev) parts.push(`sample ${ev.Refs.sample} · confidence ${ev.Refs.confidence.toFixed(1)}`)
    else if ('Call' in ev) parts.push(`at ${ev.Call.site}`)
    else if ('Manifest' in ev) parts.push(`${ev.Manifest.from} → ${ev.Manifest.to}`)
    else if ('Process' in ev) parts.push(`at ${ev.Process.site}`)
    else if ('Bindings' in ev) parts.push(`${ev.Bindings.generator} → ${ev.Bindings.output_dir}`)
  }
  const reason = inferredReason(e)
  if (reason) parts.push(`inferred: ${reason}`)
  return parts.join(' · ')
}

export function inferredReason(e: MapEdge): string | null {
  for (const ev of e.evidence) if ('Inferred' in ev) return ev.Inferred.reason
  return null
}

/** The drawn edge label: the model's *why* then the engine-rendered count (`board anchors · 5 Cargo deps`). */
export function edgeLabel(e: MapEdge): string {
  const count = evidenceCount(e.evidence)
  return [e.label, count].filter(Boolean).join(' · ')
}

// ── the element's drawable graph (§11.3) ───────────────────────────────────────────────────────────────────────────

export type ElementGraph = {
  graph: GraphData
  /** Node ids drawn hatched (`stale`). */
  hatch: string[]
  /** Node id → child's diagram, for the thumbnail. */
  thumbs: Map<string, GraphProps>
  /** Node id → the child row (drill target, listing, marks). */
  byNode: Map<string, MapChild>
  /** Edges in the same order as `graph.edges`, with their evidence (undefined for a diagram-only edge). */
  edges: (MapEdge | undefined)[]
  /** Bundled/hidden things named in the footnote. */
  hidden: string[]
}

/** First two anchor paths joined by `·`, then `+N` (§11.3 line 2). */
export function abbrevPath(paths: readonly string[]): string {
  const uniq = [...new Set(paths)]
  const head = uniq.slice(0, 2).join(' · ')
  return uniq.length > 2 ? `${head} +${uniq.length - 2}` : head
}

/**
 * `diagram` with each node re-labelled from its child row (name · path · `+N` on bundles · `ref`), edges from
 * `MapElement.edges` (label = why · count, dashed when inferred); a structure-only element without a diagram is drawn
 * from its children. `direction` is applied by the canvas per layout pass (O8b).
 */
export function elementGraph(el: MapElement, direction: 'LR' | 'TB'): ElementGraph {
  const byNode = new Map<string, MapChild>()
  for (const c of el.children) byNode.set(c.node, c)
  const base: GraphData = el.diagram
    ? (el.diagram as GraphData)
    : { nodes: el.children.map((c) => ({ id: c.node, label: c.name })), edges: el.edges.map((e) => ({ from: e.from, to: e.to })) }
  const hatch: string[] = []
  const thumbs = new Map<string, GraphProps>()
  const nodes = base.nodes.map((n) => {
    const c = byNode.get(n.id)
    if (!c) return n
    if (c.stale) hatch.push(n.id)
    if (c.thumbnail && !c.leaf) thumbs.set(n.id, c.thumbnail)
    const bundle = c.node.startsWith('bundle:') || c.kind === 'bundle'
    return {
      ...n,
      label: c.name,
      sub: c.path ?? n.sub ?? undefined,
      tag: n.tag ?? (bundle ? `+${c.files || c.lines || ''}`.replace(/\+$/, '+more') : undefined),
      ref: c.path ?? n.ref ?? undefined,
    }
  })
  const ids = new Set(nodes.map((n) => n.id))
  const edgeMeta = new Map<string, MapEdge>()
  for (const e of el.edges) edgeMeta.set(`${e.from}\u0001${e.to}`, e)
  // the engine's edge list is the truth when present (it carries evidence); the diagram's edges fill in otherwise
  const source = el.edges.length ? el.edges.map((e) => ({ from: e.from, to: e.to })) : base.edges
  const edges: GraphData['edges'] = []
  const meta: (MapEdge | undefined)[] = []
  for (const e of source) {
    if (!ids.has(e.from) || !ids.has(e.to)) continue
    const m = edgeMeta.get(`${e.from}\u0001${e.to}`)
    const d = base.edges.find((x) => x.from === e.from && x.to === e.to)
    edges.push({ ...(d ?? {}), from: e.from, to: e.to, label: m ? edgeLabel(m) : (d?.label ?? undefined), style: m ? (m.backed ? 'solid' : 'dashed') : (d?.style ?? undefined) })
    meta.push(m)
  }
  const hidden = el.children.filter((c) => c.node.startsWith('bundle:')).map((c) => `${c.name} (${c.files})`)
  return { graph: { ...base, direction, nodes, edges }, hatch, thumbs, byNode, edges: meta, hidden }
}

/** Crumb line stats: `11 elements · 15 of 16 edges` (backed of all) or `· 16 edges`. */
export function elementStats(el: MapElement): string {
  const n = el.children.length
  const backed = el.edges.filter((e) => e.backed).length
  const edges = el.edges.length
  const parts = [`${plural(n, 'element')}`]
  if (edges) parts.push(backed === edges ? plural(edges, 'edge') : `${backed} of ${edges} edges`)
  return parts.join(' · ')
}

/** The footnote under the canvas: bundles and inferred edges, never a hairball (legend item 6). */
export function footnote(g: ElementGraph, labels: Map<string, string>): string {
  const parts: string[] = []
  if (g.hidden.length) parts.push(`${g.hidden.join(', ')} hidden`)
  const inferred = g.edges.filter((e): e is MapEdge => !!e && !e.backed)
  if (inferred.length) parts.push(`dashed = inferred (${inferred.map((e) => `${labels.get(e.from) ?? e.from} → ${labels.get(e.to) ?? e.to}`).join(', ')})`)
  return parts.join(' · ')
}

// ── §11.6 progress · §8 provenance ─────────────────────────────────────────────────────────────────────────────────

export const money = (nanos: number, currency = 'USD'): string => {
  const v = nanos / 1e9
  const sym = currency === 'USD' ? '$' : currency === 'EUR' ? '€' : currency === 'GBP' ? '£' : `${currency} `
  return `${sym}${v < 0.01 && v > 0 ? '<0.01' : v.toFixed(2)}`
}

const STAGE: Record<string, string> = { select: 'Choosing perspectives', top: 'Mapping perspectives', flow: 'Tracing flows', describe: 'Describing elements', done: 'Map ready', failed: 'Generation failed', cancelled: 'Generation cancelled' }

/** `Mapping perspectives · 14 of 61 · $0.31 · 7 calls` (Q2: the cost moves in the open; S1 folds it into the session). */
export function progressLine(p: Pick<CodeMapProgress, 'stage' | 'done' | 'total' | 'calls' | 'cost_nanos' | 'currency' | 'error'> | MapJob): string {
  const parts = [STAGE[p.stage] ?? p.stage]
  if (p.total) parts.push(`${p.done} of ${p.total}`)
  if (p.calls) parts.push(money(p.cost_nanos, p.currency), plural(p.calls, 'call'))
  if (p.error) parts.push(p.error)
  return parts.join(' · ')
}

/** Short relative time: `just now`, `3 min ago`, `2 h ago`, `4 d ago`. */
export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000))
  if (s < 45) return 'just now'
  const m = Math.round(s / 60)
  if (m < 60) return `${m} min ago`
  const h = Math.round(m / 60)
  if (h < 48) return `${h} h ago`
  return `${Math.round(h / 24)} d ago`
}

/** `written by claude-… · HEAD 5cddad8e · 3 min ago · $0.04 · 2 calls`; imported elements say so (§8). */
export function provenanceLine(origin: MapOrigin, p: MapProvenance | null, now = Date.now()): string {
  if (origin === 'Imported') return 'imported · unknown freshness'
  if (!p) return origin === 'Structure' ? 'structure view · from the index, no model yet' : ''
  const parts = [`written by ${p.model}`]
  if (p.git_head) parts.push(`HEAD ${p.git_head.slice(0, 8)}`)
  parts.push(ago(p.generated_ms, now))
  if (p.cost_nanos) parts.push(money(p.cost_nanos, p.currency))
  if (p.call_ids.length) parts.push(plural(p.call_ids.length, 'call'))
  if (p.repaired) parts.push('repaired')
  return parts.join(' · ')
}

/** Honesty chips at the top of the detail pane (§11.4). */
export function detailChips(el: MapElement): string[] {
  const chips: string[] = []
  if (el.stale) chips.push('stale')
  if (el.origin === 'Imported') chips.push('imported')
  if (el.origin === 'Structure' && !el.fields.description) chips.push('needs model')
  const inferred = el.edges.filter((e) => !e.backed).length
  if (inferred) chips.push(`inferred edges: ${inferred}`)
  if (el.provenance?.repaired) chips.push('repaired')
  return chips
}

// ── §11.5 route state ──────────────────────────────────────────────────────────────────────────────────────────────

export type MapSearch = { tab: Tab; el: string | null }

const isTab = (v: unknown): v is Tab => typeof v === 'string' && (TABS as readonly string[]).includes(v)

/** `?tab=map&el=<id>` → state; anything else → Map with no element. */
export function decodeMapSearch(s: Record<string, unknown> | null | undefined): MapSearch {
  const tab = isTab(s?.tab) ? s.tab : 'map'
  const el = typeof s?.el === 'string' && s.el ? s.el : null
  return { tab, el }
}

/** State → search params, defaults omitted (`{}` for Map with no element). */
export function encodeMapSearch(s: MapSearch): { tab?: Tab; el?: string } {
  const out: { tab?: Tab; el?: string } = {}
  if (s.tab !== 'map') out.tab = s.tab
  if (s.el) out.el = s.el
  return out
}

// ── §13.3 snapshot ─────────────────────────────────────────────────────────────────────────────────────────────────

const FENCE_MAX = 12

/** The `json-render` Graph spec of an element: ≤ 12 nodes, a `ref` on every node (the child's path or the element's first anchor). */
export function mapGraphSpec(el: MapElement): unknown {
  const g = elementGraph(el, 'LR')
  const fallback = el.anchors[0]?.path ?? el.path ?? ''
  const nodes = g.graph.nodes.slice(0, FENCE_MAX).map((n) => {
    const node: Record<string, unknown> = { id: n.id, label: n.label.slice(0, 80), ref: n.ref || fallback || n.id }
    if (n.state) node.state = n.state
    return node
  })
  const kept = new Set(nodes.map((n) => n.id as string))
  const edges = g.graph.edges
    .filter((e) => kept.has(e.from) && kept.has(e.to))
    .map((e) => {
      const edge: Record<string, unknown> = { from: e.from, to: e.to }
      if (e.label) edge.label = e.label.slice(0, 80)
      if (e.style === 'dashed') edge.style = 'dashed'
      return edge
    })
  return { root: 'g', elements: { g: { type: 'Graph', props: { nodes, edges } } } }
}

export function mapFence(el: MapElement): string {
  return '```json-render\n' + JSON.stringify(mapGraphSpec(el)) + '\n```'
}

/** The 73 §6.8 draft header for a Map action. */
export const draftHead = (root: string, element: string) => `[codemap ${root.slice(root.lastIndexOf('/') + 1)} · map · ${element}]\n`

/** The code link of an anchor (`path#La-Lb`). */
export const anchorLink = (a: { path: string; line_start: number | null; line_end: number | null }) => anchorOf(a.path, a.line_start, a.line_end)
