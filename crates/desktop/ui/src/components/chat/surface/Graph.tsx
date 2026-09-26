// Design 65 §3.4: the one renderer of ours — a typed node/edge graph laid out by ELK (in the worker) and drawn as svg.
// Colours are inline attributes from the CSS tokens, not classes, so the export (`standalone()` → PNG/SVG) carries
// them; labels are `<text>` (no `<foreignObject>`), so strict CSP and export stay simple. `GraphView` is what the
// json-render registry and the apps library both mount; `GraphSvg` is the pure drawing (tested with a fixed layout).
import { type ReactNode, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { type DiagramTokens, diagramTokens } from '@/lib/diagram-theme'
import { FONT_PX, type GraphData, LINE_PX, type Layout, LayoutTimeout, layout, structuralHash, wrapLabel } from '@/lib/graph-layout'
import { createRenderQueue, type RenderQueue } from '@/lib/render-queue'
import { subscribeDark } from '@/lib/theme'
import type { GraphState } from '@/rpc/bindings/GraphState'
import { DiagramFrame } from '../DiagramFrame'

type Node = GraphData['nodes'][number]

/** Nodes present; edges whose endpoints both exist (an open fence streams nodes after the edges that name them). */
function dropDangling(g: GraphData): GraphData {
  const ids: Record<string, true> = {}
  for (const n of g.nodes) ids[n.id] = true
  return { ...g, edges: g.edges.filter((e) => ids[e.from] && ids[e.to]) }
}

/** The state to draw: a highlighted id is `active` unless the node's own state is `danger` or `done` (65 §3.5). */
function effectiveState(n: Node, highlight?: readonly string[]): GraphState {
  const own = n.state ?? 'default'
  if (highlight?.includes(n.id) && own !== 'danger' && own !== 'done') return 'active'
  return own
}

/** Stroke, fill and opacity per state. */
function paint(state: GraphState, t: DiagramTokens): { stroke: string; strokeWidth: number; fill: string; fillOpacity: number; opacity: number } {
  switch (state) {
    case 'active':
      return { stroke: t.accent, strokeWidth: 2, fill: t.accent, fillOpacity: 0.1, opacity: 1 }
    case 'done':
      return { stroke: t.success, strokeWidth: 1.25, fill: t.success, fillOpacity: 0.12, opacity: 1 }
    case 'danger':
      return { stroke: t.danger, strokeWidth: 1.5, fill: t.danger, fillOpacity: 0.12, opacity: 1 }
    case 'muted':
      return { stroke: t.mutedForeground, strokeWidth: 1, fill: t.card, fillOpacity: 1, opacity: 0.5 }
    default:
      return { stroke: t.foreground, strokeWidth: 1, fill: t.card, fillOpacity: 1, opacity: 1 }
  }
}

/** The shape outline for `kind` in the box `(x, y, w, h)`; the label is drawn by the caller. */
function Shape({ kind, x, y, w, h, ...p }: { kind: Node['kind']; x: number; y: number; w: number; h: number } & Record<string, unknown>) {
  switch (kind ?? 'box') {
    case 'rounded':
      return <rect x={x} y={y} width={w} height={h} rx={8} {...p} />
    case 'pill':
      return <rect x={x} y={y} width={w} height={h} rx={h / 2} {...p} />
    case 'diamond':
      return <polygon points={`${x + w / 2},${y - 6} ${x + w + 10},${y + h / 2} ${x + w / 2},${y + h + 6} ${x - 10},${y + h / 2}`} {...p} />
    case 'cylinder': {
      const r = 5
      return (
        <g>
          <path d={`M${x} ${y + r} v${h - 2 * r} a${w / 2} ${r} 0 0 0 ${w} 0 v${-(h - 2 * r)}`} {...p} />
          <ellipse cx={x + w / 2} cy={y + r} rx={w / 2} ry={r} {...p} />
        </g>
      )
    }
    case 'actor':
      return (
        <g>
          <circle cx={x + w / 2} cy={y - 6} r={5} {...p} />
          <rect x={x} y={y} width={w} height={h} rx={8} {...p} />
        </g>
      )
    default:
      return <rect x={x} y={y} width={w} height={h} {...p} />
  }
}

/** Wrapped label lines centred in the box. */
function Label({ text, cx, cy, fill, t, size = FONT_PX }: { text: string; cx: number; cy: number; fill: string; t: DiagramTokens; size?: number }) {
  const lines = wrapLabel(text)
  const top = cy - ((lines.length - 1) * LINE_PX) / 2
  return (
    <text textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={size} fill={fill}>
      {lines.map((l, i) => (
        <tspan key={i} x={cx} y={top + i * LINE_PX}>
          {l}
        </tspan>
      ))}
    </text>
  )
}

/** The svg for `graph` at `layout`; `markerId` must be unique per mounted svg (arrowheads are looked up by id). */
export function GraphSvg({
  graph,
  layout: l,
  tokens: t,
  markerId,
  highlight,
  onSelect,
}: {
  graph: GraphData
  layout: Layout
  tokens: DiagramTokens
  markerId: string
  highlight?: readonly string[]
  onSelect?: (id: string) => void
}) {
  const M = 12
  const W = Math.ceil(l.width + 2 * M)
  const H = Math.ceil(l.height + 2 * M)
  const nodes: Record<string, Node> = {}
  for (const n of graph.nodes) nodes[n.id] = n
  const groups: Record<string, string> = {}
  for (const g of graph.groups ?? []) groups[g.id] = g.label
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} role="img" aria-label="Graph">
      <defs>
        <marker id={markerId} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M0 0 L10 5 L0 10 z" fill={t.foreground} />
        </marker>
      </defs>
      <g transform={`translate(${M} ${M})`}>
        {l.groups.map((g) => (
          <g key={g.id}>
            <rect x={g.x} y={g.y} width={g.w} height={g.h} rx={8} fill={t.muted} fillOpacity={0.35} stroke={t.border} />
            <text x={g.x + 12} y={g.y + 16} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.mutedForeground}>
              {groups[g.id] ?? g.id}
            </text>
          </g>
        ))}
        {l.edges.map((e) => {
          const edge = graph.edges[e.index]
          if (!edge) return null
          const d = e.sections.map((s) => [s.start, ...s.bends, s.end].map((p, i) => `${i ? 'L' : 'M'}${p.x} ${p.y}`).join(' ')).join(' ')
          return (
            <g key={e.index}>
              <path d={d} fill="none" stroke={t.foreground} strokeWidth={1.25} strokeDasharray={edge.style === 'dashed' ? '6 4' : undefined} markerEnd={`url(#${markerId})`} />
              {e.label && edge.label && (
                <g>
                  <rect x={e.label.x} y={e.label.y} width={e.label.w} height={e.label.h} rx={3} fill={t.background} />
                  <Label text={edge.label} cx={e.label.x + e.label.w / 2} cy={e.label.y + e.label.h / 2} fill={t.mutedForeground} t={t} size={FONT_PX - 1} />
                </g>
              )}
            </g>
          )
        })}
        {l.nodes.map((n) => {
          const node = nodes[n.id]
          if (!node) return null
          const s = paint(effectiveState(node, highlight), t)
          return (
            <g
              key={n.id}
              opacity={s.opacity}
              data-node={n.id}
              {...(onSelect ? { role: 'button', tabIndex: 0, style: { cursor: 'pointer' }, onClick: () => onSelect(n.id), onKeyDown: (ev: React.KeyboardEvent) => (ev.key === 'Enter' || ev.key === ' ') && onSelect(n.id) } : {})}
            >
              <Shape kind={node.kind} x={n.x} y={n.y} w={n.w} h={n.h} fill={s.fill} fillOpacity={s.fillOpacity} stroke={s.stroke} strokeWidth={s.strokeWidth} />
              <Label text={node.label} cx={n.x + n.w / 2} cy={n.y + n.h / 2} fill={t.foreground} t={t} />
            </g>
          )
        })}
      </g>
    </svg>
  )
}

const GLYPH: Record<GraphState, string> = { active: '◉', done: '●', muted: '○', danger: '✗', default: '•' }

/** `crates/surface::graph::adjacency_lines` in TS: what the card shows when the layout times out. */
export function adjacencyText(g: GraphData): string {
  const label: Record<string, string> = {}
  for (const n of g.nodes) label[n.id] = n.label
  const line = (n: Node, indent: string) => {
    const targets = g.edges.filter((e) => e.from === n.id).map((e) => (e.label ? `${label[e.to] ?? e.to} (${e.label})` : (label[e.to] ?? e.to)))
    return `${indent}${GLYPH[n.state ?? 'default']} ${n.label}${targets.length ? ` → ${targets.join(', ')}` : ''}`
  }
  const groups = g.groups ?? []
  const known: Record<string, true> = {}
  for (const grp of groups) known[grp.id] = true
  const out: string[] = []
  for (const grp of groups) {
    out.push(grp.label)
    for (const n of g.nodes) if (n.group === grp.id) out.push(line(n, '  '))
  }
  for (const n of g.nodes) if (!(n.group && known[n.group])) out.push(line(n, ''))
  return out.join('\n')
}

type Shown = { graph: GraphData; hash: string; layout: Layout } | { graph: GraphData; hash: string; timeout: true }

export interface GraphViewProps {
  props: GraphData
  /** The fence is still streaming: edges naming absent nodes are dropped instead of drawn. */
  loading?: boolean
  /** Node ids drawn `active` (apps, 65 §3.5). */
  highlight?: readonly string[]
  /** A node was pressed (apps). */
  onSelect?: (id: string) => void
}

/**
 * The Graph card: layout through the render queue (coalesced while streaming, last good layout stays on screen),
 * the svg in the shared `DiagramFrame`, the adjacency list when ELK exceeds 2 s — never a blank card.
 */
export function GraphView({ props, loading, highlight, onSelect }: GraphViewProps) {
  const graph = useMemo(() => (loading ? dropDangling(props) : props), [props, loading])
  const hash = useMemo(() => structuralHash(graph), [graph])
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'))
  const tokens = useMemo(diagramTokens, [dark])
  const [shown, setShown] = useState<Shown | null>(null)
  const queue = useRef<RenderQueue<{ graph: GraphData; hash: string }> | null>(null)
  useEffect(() => {
    const q = createRenderQueue<{ graph: GraphData; hash: string }>(async (req, isCurrent) => {
      try {
        const l = await layout(req.graph, req.hash)
        if (isCurrent()) setShown({ ...req, layout: l })
      } catch (e) {
        if (isCurrent() && e instanceof LayoutTimeout) setShown({ ...req, timeout: true })
      }
    })
    queue.current = q
    return () => q.dispose()
  }, [])
  useEffect(() => {
    queue.current?.request({ graph, hash })
  }, [graph, hash])

  let content: ReactNode
  if (shown && 'layout' in shown) {
    content = <GraphSvg graph={shown.graph} layout={shown.layout} tokens={tokens} markerId={`arrow-${shown.hash}`} highlight={highlight} onSelect={onSelect} />
  } else if (shown) {
    content = <pre className="p-3 font-mono text-xs leading-5 whitespace-pre-wrap">{adjacencyText(shown.graph)}</pre>
  }
  return (
    <DiagramFrame name={`graph-${hash}`} label="graph" kind="graph-block" title={shown && 'timeout' in shown ? 'layout timed out; showing the list' : undefined}>
      {content}
    </DiagramFrame>
  )
}
