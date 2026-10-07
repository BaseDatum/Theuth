// Design 123 §0.9, §4.2, §5.2(a) (W1): `GraphDelta` — the UNION of `before` and `after` is laid out once (the Graph's
// `toElk`), then two panels draw each side's subset at the union coordinates, so an unchanged node keeps its place
// (ADR 0012). The ledger (added / removed / changed when label, kind or group differ) is derived by id here and in
// `crates/surface::elk_family::ledger`, never authored.
import { type ReactNode, useEffect, useMemo, useRef } from 'react'
import type { z } from 'zod'
import { describe, svgA11y } from '@/lib/diagram/a11y'
import { fnv, norm } from '@/lib/diagram/elk'
import type { DiagramHostProps } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { flattenElk, FONT_PX, type GraphData, type Layout, structuralHash as graphHash, toElk, useDiagramTokens, useElkLayout } from '@/lib/graph-layout'
import { adjacencyText, ArrowMarkers, dropDangling as dropGraph, ElkFrame, GraphBody, graphTooltip, MARGIN, MONO, paint, TITLE_H, useGraphUnits } from '../Graph'
import type { graphDeltaProps } from '../catalog'

export type GraphDeltaData = z.infer<typeof graphDeltaProps>
/** One side in the loose (wire) shape `GraphData` uses; the zod `GraphDeltaData` sides are assignable to it. */
type Side = Omit<GraphData, 'direction' | 'legend' | 'title'>
type Node = GraphData['nodes'][number]
type Edge = GraphData['edges'][number]
/** The props in the loose shape every function here takes (`GraphDeltaData` is assignable). */
export type Delta = { before: Side; after: Side; direction?: GraphData['direction']; title?: string | null }

/** The gap between the two panels and the height of their `Before` / `After` headers. */
const PANEL_GAP = 24
const PANEL_HEAD = 20
const LEDGER_H = 20

export interface Ledger {
  added: string[]
  removed: string[]
  changed: { id: string; what: string }[]
}

/** The derived ledger (twin of the Rust one): by id, `what` ⊆ `label, kind, group` in that order. */
export function ledger(p: Delta): Ledger {
  const before: Record<string, Node> = {}
  for (const n of p.before.nodes) before[n.id] = n
  const after: Record<string, true> = {}
  for (const n of p.after.nodes) after[n.id] = true
  const out: Ledger = { added: [], removed: [], changed: [] }
  for (const n of p.after.nodes) {
    const b = before[n.id]
    if (!b) {
      out.added.push(n.id)
      continue
    }
    const what = [b.label !== n.label && 'label', (b.kind ?? null) !== (n.kind ?? null) && 'kind', (b.group ?? null) !== (n.group ?? null) && 'group'].filter(Boolean)
    if (what.length) out.changed.push({ id: n.id, what: what.join(', ') })
  }
  for (const n of p.before.nodes) if (!after[n.id]) out.removed.push(n.id)
  return out
}

const pair = (e: Edge) => `${e.from}\u0001${e.to}`

/**
 * The union graph: nodes by id (`after` wins for label/kind/group, `before`-only nodes follow), edges by
 * (from, to) pair in `after` order then `before`'s extras, groups by id. Its layout places both panels.
 */
export function union(p: Delta): GraphData {
  const nodes: Node[] = [...p.after.nodes]
  const seen: Record<string, true> = {}
  for (const n of nodes) seen[n.id] = true
  for (const n of p.before.nodes) if (!seen[n.id]) nodes.push(n)
  const edges: Edge[] = [...p.after.edges]
  const pairs: Record<string, true> = {}
  for (const e of edges) pairs[pair(e)] = true
  for (const e of p.before.edges) if (!pairs[pair(e)]) edges.push(e)
  const groups = [...(p.after.groups ?? [])]
  const gids: Record<string, true> = {}
  for (const g of groups) gids[g.id] = true
  for (const g of p.before.groups ?? []) if (!gids[g.id]) groups.push(g)
  return { nodes, edges, groups: groups.length ? groups : undefined, direction: p.direction }
}

/** One side as the `GraphData` its panel draws: its own nodes/groups, its edges at the union's indices. */
export function sideGraph(side: Side, u: GraphData): GraphData {
  const own: Record<string, Edge> = {}
  for (const e of side.edges) own[pair(e)] = e
  return { nodes: side.nodes, edges: u.edges.map((e) => own[pair(e)] ?? e), groups: side.groups, direction: u.direction }
}

/** The union layout cut to one side: its nodes, groups and the edges it has (ids and indices unchanged). */
export function sideLayout(side: Side, u: GraphData, l: Layout): Layout {
  const ids: Record<string, true> = {}
  for (const n of side.nodes) ids[n.id] = true
  const gids: Record<string, true> = {}
  for (const g of side.groups ?? []) gids[g.id] = true
  const pairs: Record<string, true> = {}
  for (const e of side.edges) pairs[pair(e)] = true
  return { ...l, nodes: l.nodes.filter((n) => ids[n.id]), groups: l.groups.filter((g) => gids[g.id]), edges: l.edges.filter((e) => u.edges[e.index] && pairs[pair(u.edges[e.index]!)]) }
}

export function dropDangling(p: Delta): Delta {
  return { ...p, before: dropGraph({ ...p.before, direction: p.direction }), after: dropGraph({ ...p.after, direction: p.direction }) }
}

export function structuralHash(p: Delta): string {
  return fnv(`gd:${norm(p.title)}:${graphHash({ ...p.before, direction: p.direction })}:${graphHash({ ...p.after, direction: p.direction })}`)
}

/** `Before: n nodes · After: m nodes`, the ledger, then the `after` adjacency list — the TUI text, for the time-out. */
export function graphDeltaText(p: Delta): string {
  const label = (id: string) => [...p.after.nodes, ...p.before.nodes].find((n) => n.id === id)?.label ?? id
  const l = ledger(p)
  const lines = [...l.added.map((id) => `+ ${label(id)}`), ...l.removed.map((id) => `− ${label(id)}`), ...l.changed.map((c) => `~ ${label(c.id)} (${c.what})`)]
  return [p.title, `Before: ${p.before.nodes.length} nodes · After: ${p.after.nodes.length} nodes`, ...(lines.length ? lines : ['= no node changes']), '', adjacencyText({ ...p.after, direction: p.direction })].filter((x) => x !== undefined).join('\n')
}

/** Two panels over one layout plus the ledger strip. Unit ids are shared by both panels (a node lights on both sides). */
export function GraphDeltaSvg({ props: p, layout: l, tokens: t, markerId, units, root }: { props: Delta; layout: Layout; tokens: DiagramTokens; markerId: string; units?: Units; root?: { onKeyDown: (e: React.KeyboardEvent) => void } }) {
  const u = union(p)
  const panelW = Math.ceil(l.width)
  const panelH = Math.ceil(l.height)
  const top = MARGIN + (p.title ? TITLE_H : 0)
  const W = Math.ceil(2 * MARGIN + 2 * panelW + PANEL_GAP)
  const led = ledger(p)
  const rows = led.added.length + led.removed.length + led.changed.length
  const H = Math.ceil(top + PANEL_HEAD + panelH + MARGIN + (rows ? LEDGER_H * rows + 8 : 0))
  const a = svgA11y(markerId, !!units)
  const label = (id: string) => u.nodes.find((n) => n.id === id)?.label ?? id
  const panel = (name: string, side: Side, x: number) => (
    <g transform={`translate(${x} ${top})`} data-panel={name.toLowerCase()}>
      <text x={0} y={10} fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
        {name}
      </text>
      <rect x={-6} y={PANEL_HEAD - 4} width={panelW + 12} height={panelH + 8} rx={8} fill="none" stroke={t.border} strokeDasharray="2 4" />
      <g transform={`translate(0 ${PANEL_HEAD})`}>
        <GraphBody graph={sideGraph(side, u)} layout={sideLayout(side, u, l)} tokens={t} markerId={markerId} units={units} />
      </g>
    </g>
  )
  const ledgerY = top + PANEL_HEAD + panelH + 12
  const entries = [...led.added.map((id) => ({ id, glyph: '+', text: label(id), color: paint('done', t).stroke })), ...led.removed.map((id) => ({ id, glyph: '−', text: label(id), color: paint('danger', t).stroke })), ...led.changed.map((c) => ({ id: c.id, glyph: '~', text: `${label(c.id)} (${c.what})`, color: paint('active', t).stroke }))]
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...root}>
      <title id={a.titleId}>{p.title ?? 'GraphDelta'}</title>
      <desc id={a.descId}>{describe('GraphDelta', p.title, p.after.nodes.length, 'nodes')}</desc>
      <defs>
        <ArrowMarkers id={markerId} tokens={t} />
      </defs>
      {p.title && (
        <text x={MARGIN} y={MARGIN + 13} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
          {p.title}
        </text>
      )}
      {panel('Before', p.before, MARGIN)}
      {panel('After', p.after, MARGIN + panelW + PANEL_GAP)}
      <g data-ledger fontFamily={MONO} fontSize={11}>
        {entries.map((e, i) => (
          <text key={`${e.glyph}${e.id}`} x={MARGIN} y={ledgerY + i * LEDGER_H + 11} fill={e.color}>
            {e.glyph} {e.text}
          </text>
        ))}
      </g>
    </svg>
  )
}

export function GraphDeltaView({ props, loading, highlight, onSelect }: DiagramHostProps<GraphDeltaData>) {
  const p = useMemo(() => (loading ? dropDangling(props) : props), [props, loading])
  const hash = useMemo(() => structuralHash(p), [p])
  const tokens = useDiagramTokens()
  const shown = useElkLayout(hash, () => toElk(union(p)), flattenElk)
  const drawnRef = useRef<Delta>(p)
  const drawn = shown && shown.hash === hash ? p : drawnRef.current
  useEffect(() => {
    drawnRef.current = drawn
  }, [drawn])
  const u = useMemo(() => union(drawn), [drawn])
  const ids = useMemo(() => u.nodes.map((n) => n.id), [u])
  const units = useGraphUnits(ids, u.edges, highlight, onSelect)
  let content: ReactNode
  if (shown && 'layout' in shown) content = <GraphDeltaSvg props={drawn} layout={shown.layout} tokens={tokens} markerId={`gd-${shown.hash}`} units={units} root={units.root} />
  else if (shown) content = <pre className="p-3 font-mono text-xs leading-5 whitespace-pre-wrap">{graphDeltaText(drawn)}</pre>
  return (
    <ElkFrame kind="graphdelta" hash={hash} timeout={!!shown && 'timeout' in shown} hover={units.state.hover} tip={graphTooltip(u, units.state.hover)}>
      {content}
    </ElkFrame>
  )
}
