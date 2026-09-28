/** 73 §6.6 — hand layouts for Fit (columns) and Change (bands), pure, < 1 ms; same `Layout` shape as `graph-layout.ts`. */
import { FONT_PX, LINE_PX, wrapLabel, type GraphData, type Layout, type LayoutEdge, type LayoutGroup, type LayoutNode } from '@/lib/graph-layout'
import { anchorOf, BADGE, GRAPH_STATE, kindGlyph, shortLabels, type ColourState } from './labels'
import type { ChangeLens, FitLens, FitNode } from './types'

export type Slot = { layer: number; index: number; w: number; h: number }
export type Slots = Map<string, Slot>
/** What a lens draws: ids = moniker; file nodes `<lang>:<path>`; bundle nodes `bundle:<layer>`. */
export type Placed = { graph: GraphData; layout: Layout; badges: Record<string, string>; hatch: string[] }
export const NODE_GAP = 24
export const BAND_GAP = 40
const CHAR_PX = FONT_PX * 0.62
const PAD = 12
const TITLE = 28
/** Badge words sit above a pill's top-right corner (mono 11 px): the first row starts below them and a pill is never narrower than its badge. */
const BADGE_H = 14
const BADGE_PX = 6.8
type N = GraphData['nodes'][number]

export function nodeSize(label: string): { w: number; h: number } {
  const lines = wrapLabel(label)
  return { w: Math.max(64, Math.ceil(Math.max(...lines.map((l) => l.length)) * CHAR_PX + 24)), h: Math.max(32, lines.length * LINE_PX + 16) }
}

/** 73 §6.6 rules 2–4: an id keeps its slot for the life of the map; a new id takes the first index no slot of its layer holds. */
export function placeSlot(slots: Slots, id: string, layer: number, w = 0, h = 0): Slot {
  let s = slots.get(id)
  if (s) return s
  const used: Record<number, true> = {}
  for (const t of slots.values()) if (t.layer === layer) used[t.index] = true
  let index = 0
  while (used[index]) index++
  s = { layer, index, w, h }
  slots.set(id, s)
  return s
}

/** Every slot of `layer` (removed ids included — their reserved size keeps later slots where they were), by index. */
const layerSlots = (slots: Slots, layer: number) => [...slots.values()].filter((s) => s.layer === layer).sort((a, b) => a.index - b.index)

const symLabel = (n: FitNode) => `${kindGlyph(n.kind)} ${n.name}`.trim()

const withStale = (badges: Record<string, string>, hatch: string[], id: string) => {
  hatch.push(id)
  badges[id] = badges[id] ? `${badges[id]} · ${BADGE.stale}` : BADGE.stale
}

export function columnsLayout(fit: FitLens, slots: Slots, width: number, opts: { stale?: ReadonlySet<string>; limit?: number } = {}): Placed {
  const callers = fit.callers.slice()
  const callees = fit.callees.slice()
  const more = [fit.more_callers, fit.more_callees]
  // 73 §6.5: a cut is never silent — beyond `limit` the bundle carries the count
  for (let n = callers.length + callees.length; n > (opts.limit ?? 20) - 1; n--) {
    const i = callers.length >= callees.length ? 0 : 1
    ;(i ? callees : callers).pop()
    more[i]!++
  }
  const groupBy = (items: FitNode[]) => {
    const per: Record<string, FitNode[]> = {}
    for (const x of items) (per[x.path] ??= []).push(x)
    return per
  }
  const per = [groupBy(callers), groupBy(callees)]
  // 73 §6.5: file nodes show the shortest distinguishing suffix among the file nodes on screen
  const short = shortLabels(per.flatMap((p) => Object.keys(p).filter((k) => p[k]!.length > 1)))
  const alias: Record<string, string> = { [fit.focus.moniker]: fit.focus.moniker }
  const badges: Record<string, string> = {}
  const hatch: string[] = []
  const drawn: { node: N; layer: number }[] = []
  const column = (items: FitNode[], layer: number) => {
    // 73 §6.5 "text is the map": a name that repeats in a column carries its file's shortest distinguishing suffix
    const names: Record<string, number> = {}
    for (const x of items) names[x.name] = (names[x.name] ?? 0) + 1
    const paths = shortLabels(items.map((x) => x.path))
    for (const x of items) {
      const g = per[layer / 2]![x.path]!
      const id = g.length > 1 ? `${x.moniker.slice(0, x.moniker.indexOf(':'))}:${x.path}` : x.moniker
      alias[x.moniker] = id
      if (g[0] !== x) continue
      const test = g.some((y) => y.is_test)
      if (test) badges[id] = BADGE.test
      const label = g.length > 1 ? `${short.get(x.path)} ×${g.length}` : names[x.name]! > 1 ? `${symLabel(x)} ${paths.get(x.path)}` : symLabel(x)
      drawn.push({ node: { id, label, state: test ? GRAPH_STATE.test : undefined, ref: g.length > 1 ? x.path : anchorOf(x.path, x.line_start, x.line_end) }, layer })
    }
  }
  column(callers, 0)
  badges[fit.focus.moniker] = fit.ambiguous > 0 ? `${BADGE.impacted} · ?${fit.ambiguous}` : BADGE.impacted
  drawn.push({ node: { id: fit.focus.moniker, label: symLabel(fit.focus), state: GRAPH_STATE.impacted, ref: anchorOf(fit.focus.path, fit.focus.line_start, fit.focus.line_end) }, layer: 1 })
  column(callees, 2)
  for (const d of drawn) {
    const { w, h } = nodeSize(d.node.label)
    placeSlot(slots, d.node.id, d.layer, w, h)
    if (opts.stale?.has(d.node.id)) withStale(badges, hatch, d.node.id)
  }

  const bundles = [0, 2].map((layer) => (more[layer / 2]! > 0 ? { id: `bundle:${layer}`, label: `+${more[layer / 2]} ${layer ? 'callees' : 'callers'}` } : null))
  const cols = [0, 1, 2].map((l) => layerSlots(slots, l))
  const stack = cols.map((c) => c.reduce((a, s) => a + s.h + NODE_GAP, 0))
  const bundleSize = bundles.map((b) => (b ? nodeSize(b.label) : null))
  const height = Math.max(stack[0]! + (bundleSize[0]?.h ?? -NODE_GAP), stack[2]! + (bundleSize[1]?.h ?? -NODE_GAP), stack[1]! - NODE_GAP)
  const xOf = (layer: number, w: number) => (layer === 0 ? 0 : layer === 1 ? (width - w) / 2 : width - w)
  const yAt = cols.map((c) => {
    const at: Record<number, number> = {}
    let y = 0
    for (const s of c) {
      at[s.index] = y
      y += s.h + NODE_GAP
    }
    return at
  })
  const box: Record<string, LayoutNode> = {}
  const nodes: N[] = []
  const lnodes: LayoutNode[] = []
  for (const d of drawn) {
    const s = slots.get(d.node.id)!
    const y = d.layer === 1 ? (height - s.h) / 2 : yAt[d.layer]![s.index]!
    box[d.node.id] = { id: d.node.id, x: xOf(d.layer, s.w), y, w: s.w, h: s.h }
    nodes.push(d.node)
    lnodes.push(box[d.node.id]!)
  }
  bundles.forEach((b, i) => {
    if (!b) return
    const layer = i * 2
    const { w, h } = bundleSize[i]!
    nodes.push({ id: b.id, label: b.label, kind: 'pill', state: 'muted' })
    lnodes.push({ id: b.id, x: xOf(layer, w), y: stack[layer]!, w, h })
  })

  const merged: Record<string, { from: string; to: string; count: number; confidence: number }> = {}
  for (const e of fit.edges) {
    const from = alias[e.from]
    const to = alias[e.to]
    if (!from || !to || from === to || !box[from] || !box[to]) continue
    const m = (merged[`${from}\u0000${to}`] ??= { from, to, count: 0, confidence: 1 })
    m.count += e.count
    m.confidence = Math.min(m.confidence, e.confidence)
  }
  const edges: GraphData['edges'] = []
  const ledges: LayoutEdge[] = []
  for (const m of Object.values(merged)) {
    const a = box[m.from]!
    const b = box[m.to]!
    const lr = a.x <= b.x
    const start = { x: lr ? a.x + a.w : a.x, y: a.y + a.h / 2 }
    const end = { x: lr ? b.x : b.x + b.w, y: b.y + b.h / 2 }
    const mid = (start.x + end.x) / 2
    const label = m.count > 1 ? `×${m.count}` : undefined
    const le: LayoutEdge = { index: edges.length, sections: [{ start, bends: [{ x: mid, y: start.y }, { x: mid, y: end.y }], end }] }
    if (label) {
      const w = Math.ceil(label.length * CHAR_PX + 8)
      const h = LINE_PX + 2
      le.label = { x: mid - w / 2, y: (start.y + end.y) / 2 - h / 2, w, h }
    }
    edges.push({ from: m.from, to: m.to, label, style: m.confidence < 1 ? 'dashed' : undefined })
    ledges.push(le)
  }
  return { graph: { nodes, edges }, layout: { nodes: lnodes, edges: ledges, groups: [], width, height }, badges, hatch }
}

const BANDS = ['changed', 'direct', 'transitive', 'tests'] as const
const BAND_STATE: ColourState[] = ['changed', 'impacted', 'transitive', 'test']
const STATUS: Record<string, string> = { modified: '~', added: '+', deleted: '−' }
const LINK: Record<string, string> = { ref: '✓', subject: '◇' }

export function bandsLayout(change: ChangeLens, slots: Slots, width: number, opts: { caps?: [number, number, number, number] } = {}): Placed {
  const caps = opts.caps ?? [12, 16, 16, 16]
  const bands = BANDS.map((b) => change[b])
  const short = shortLabels(bands.flat().map((f) => f.path))
  const stale: Record<string, true> = {}
  for (const p of change.unindexed) stale[p] = true
  const nodes: N[] = []
  const lnodes: LayoutNode[] = []
  const groups: LayoutGroup[] = []
  const badges: Record<string, string> = {}
  const hatch: string[] = []
  const seen: Record<string, true> = {}
  let top = 0
  bands.forEach((files, layer) => {
    const band = BANDS[layer]!
    const kept = files.slice(0, caps[layer])
    const drawn: N[] = []
    for (const f of kept) {
      if (seen[f.path]) continue
      seen[f.path] = true
      const prefix = layer === 0 ? (STATUS[f.status ?? ''] ?? '') : layer === 3 ? (LINK[f.test_link ?? ''] ?? '') : ''
      const n: N = { id: f.path, label: `${prefix} ${short.get(f.path)}`.trim(), kind: 'pill', group: band, state: GRAPH_STATE[BAND_STATE[layer]!], ref: f.path }
      badges[f.path] = BADGE[BAND_STATE[layer]!]
      if (stale[f.path]) withStale(badges, hatch, f.path)
      const { w: lw, h } = nodeSize(n.label)
      const w = Math.max(lw, Math.ceil((badges[f.path]?.length ?? 0) * BADGE_PX) + 8)
      placeSlot(slots, f.path, layer, w, h)
      drawn.push(n)
    }
    // Flow over every slot of the layer (reserved ones too): a pill's place depends only on the slots before it
    const at: Record<number, { x: number; y: number }> = {}
    let x = PAD
    let y = top + TITLE + BADGE_H
    let row = 0
    const step = (w: number, h: number) => {
      if (x > PAD && x + w > width - PAD) {
        x = PAD
        y += row + NODE_GAP
        row = 0
      }
      const p = { x, y }
      row = Math.max(row, h)
      x += w + NODE_GAP
      return p
    }
    for (const s of layerSlots(slots, layer)) at[s.index] = step(s.w, s.h)
    for (const n of drawn) {
      const s = slots.get(n.id)!
      nodes.push(n)
      lnodes.push({ id: n.id, ...at[s.index]!, w: s.w, h: s.h })
    }
    // 73 §6.5: a cut is never silent — the bundle node carries the count
    const over = files.length - kept.length + change.more[layer]!
    if (over > 0) {
      const dirs: Record<string, number> = {}
      for (const f of files) {
        const d = f.path.split('/').slice(0, 2).join('/')
        dirs[d] = (dirs[d] ?? 0) + 1
      }
      const dir = Object.keys(dirs).sort((a, b) => dirs[b]! - dirs[a]!)[0]
      const id = `bundle:${layer}`
      const label = dir ? `+${over} in ${dir}` : `+${over}`
      const { w, h } = nodeSize(label)
      nodes.push({ id, label, kind: 'pill', group: band, state: 'muted' })
      lnodes.push({ id, ...step(w, h), w, h })
    }
    const h = y + row - top + PAD
    groups.push({ id: band, x: 0, y: top, w: width, h })
    top += h + BAND_GAP
  })
  const graph: GraphData = { nodes, edges: [], groups: BANDS.map((b, i) => ({ id: b, label: `${b} (${change[b].length + change.more[i]!})` })) }
  return { graph, layout: { nodes: lnodes, edges: [], groups, width, height: top - BAND_GAP }, badges, hatch }
}
