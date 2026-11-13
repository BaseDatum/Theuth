// Design 65 §3.4 / 123 §4.1, §5.3, §5.4: the one renderer of ours — a typed node/edge graph laid out by ELK (in the
// worker) and drawn as svg. Colours are inline attributes from the CSS tokens, not classes, so the export
// (`standalone()` → PNG/SVG) carries them; labels are `<text>` (no `<foreignObject>`). `GraphView` is what the
// json-render registry, the boards and the apps library mount; `GraphSvg` is the pure drawing (tested with a fixed
// layout); `GraphBody` is the inner `<g>` GraphDelta draws twice. The pieces every ELK-family component shares
// (`paint`, `Shape`, `Label`, `NodeIcon`, `ArrowMarkers`, `EdgePath`, `edgeArrows`) are exported from here.
import { AppWindow, Bot, Box, Clock, Cloud, Code, Cpu, Database, File, Folder, GitBranch, Globe, Key, Laptop, ListOrdered, Lock, type LucideIcon, Mail, Package, Phone, Server, Settings, Shield, Terminal, User, Users } from 'lucide-react'
import { type KeyboardEvent, type ReactNode, useCallback, useEffect, useMemo, useRef } from 'react'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { MaskedLabel } from '@/lib/diagram/label'
import { LEGEND_H, Legend, type LegendItem } from '@/lib/diagram/legend'
import { type Arrow, markers, pointAt, sectionsPath } from '@/lib/diagram/path'
import { DiagramTooltip } from '@/lib/diagram/tooltip'
import type { Point, UnitState } from '@/lib/diagram/types'
import { focusUnit, type UnitAttrs, type Units, useUnits } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { flattenElk, FONT_PX, type GraphData, ICON_W, type Layout, type LayoutEdge, LINE_PX, structuralHash, toElk, useDiagramTokens, useElkLayout, wrapLabel } from '@/lib/graph-layout'
import type { GraphState } from "@/rpc/bindings/method.codeindex.output"
import { DiagramFrame } from '../DiagramFrame'

type Node = GraphData['nodes'][number]
type Edge = GraphData['edges'][number]

/** Nodes present; edges whose endpoints both exist (an open fence streams nodes after the edges that name them). */
export function dropDangling(g: GraphData): GraphData {
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

export interface Paint {
  stroke: string
  strokeWidth: number
  fill: string
  fillOpacity: number
  opacity: number
}

/** Stroke, fill and opacity per state (§5.4 #11: fills are state tints 0.10–0.12). */
export function paint(state: UnitState | null | undefined, t: DiagramTokens): Paint {
  switch (state ?? 'default') {
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
export function Shape({ kind, x, y, w, h, ...p }: { kind: Node['kind'] | 'parallelogram'; x: number; y: number; w: number; h: number } & Record<string, unknown>) {
  switch (kind ?? 'box') {
    case 'rounded':
      return <rect x={x} y={y} width={w} height={h} rx={8} {...p} />
    case 'pill':
      return <rect x={x} y={y} width={w} height={h} rx={h / 2} {...p} />
    case 'diamond':
      return <polygon points={`${x + w / 2},${y - 6} ${x + w + 10},${y + h / 2} ${x + w / 2},${y + h + 6} ${x - 10},${y + h / 2}`} {...p} />
    case 'hexagon': {
      const k = Math.min(12, h / 2)
      return <polygon points={`${x + k},${y} ${x + w - k},${y} ${x + w},${y + h / 2} ${x + w - k},${y + h} ${x + k},${y + h} ${x},${y + h / 2}`} {...p} />
    }
    case 'circle': {
      const r = Math.max(w, h) / 2
      return <circle cx={x + w / 2} cy={y + h / 2} r={r} {...p} />
    }
    case 'note': {
      const f = 10
      return <path d={`M${x} ${y} H${x + w - f} L${x + w} ${y + f} V${y + h} H${x} Z M${x + w - f} ${y} V${y + f} H${x + w}`} {...p} />
    }
    case 'parallelogram': {
      const k = 10
      return <polygon points={`${x + k},${y} ${x + w},${y} ${x + w - k},${y + h} ${x},${y + h}`} {...p} />
    }
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

export const MONO = 'ui-monospace, SFMono-Regular, Menlo, monospace'

/** Wrapped label lines centred at `(cx, cy)`; `weight` 600 for names (§5.4 #10). */
export function Label({ text, cx, cy, fill, t, size = FONT_PX, weight, mono }: { text: string; cx: number; cy: number; fill: string; t: DiagramTokens; size?: number; weight?: number; mono?: boolean }) {
  const lines = wrapLabel(text)
  const top = cy - ((lines.length - 1) * LINE_PX) / 2
  return (
    <text textAnchor="middle" dominantBaseline="central" fontFamily={mono ? MONO : t.font} fontSize={size} fontWeight={weight} fill={fill}>
      {lines.map((l, i) => (
        <tspan key={i} x={cx} y={top + i * LINE_PX}>
          {l}
        </tspan>
      ))}
    </text>
  )
}

/** `GraphIcon` → lucide (123 §4.1); `queue` and `browser` have no lucide twin and map to list-ordered / app-window. */
const ICONS: Record<NonNullable<Node['icon']>, LucideIcon> = {
  server: Server,
  database: Database,
  cloud: Cloud,
  user: User,
  users: Users,
  globe: Globe,
  lock: Lock,
  shield: Shield,
  key: Key,
  queue: ListOrdered,
  cpu: Cpu,
  box: Box,
  package: Package,
  file: File,
  folder: Folder,
  terminal: Terminal,
  laptop: Laptop,
  phone: Phone,
  mail: Mail,
  clock: Clock,
  settings: Settings,
  code: Code,
  'git-branch': GitBranch,
  bot: Bot,
  browser: AppWindow,
}

/** A 16 px lucide glyph as a nested `<svg>` (inline paths, never `<foreignObject>`), top-left at `(x, y)`. */
export function NodeIcon({ icon, x, y, color }: { icon: Node['icon']; x: number; y: number; color: string }) {
  const I = icon ? ICONS[icon] : undefined
  if (!I) return null
  return <I x={x} y={y} width={16} height={16} color={color} strokeWidth={1.75} aria-hidden="true" data-icon={icon} />
}

const STATES: readonly UnitState[] = ['default', 'active', 'done', 'muted', 'danger']

/** One arrowhead marker per state colour: `${id}` (default) and `${id}-${state}`; `markerFor` names them. */
export function ArrowMarkers({ id, tokens: t }: { id: string; tokens: DiagramTokens }) {
  return (
    <>
      {STATES.map((s) => (
        <marker key={s} id={s === 'default' ? id : `${id}-${s}`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M0 0 L10 5 L0 10 z" fill={paint(s, t).stroke} />
        </marker>
      ))}
    </>
  )
}

export function markerFor(markerId: string | null, state: UnitState): string | null {
  return markerId === null ? null : state === 'default' ? markerId : `${markerId}-${state}`
}

/**
 * Where an edge's label sits: on the run ELK reserved room beside (`e.label`, its placed box — the segment nearest
 * that box's centre, the mask on the box's side of the stroke), else the longest straight run's midpoint, before it.
 * Either way the mask is `LABEL_GAP` off the stroke (§5.4 #2); ELK's side keeps it off the nodes (§5.4 #5).
 */
export function edgeLabelAt(e: Pick<LayoutEdge, 'sections' | 'label'>): { at: Point; along: 'horizontal' | 'vertical'; side: 'before' | 'after' } {
  const c = e.label ? { x: e.label.x + e.label.w / 2, y: e.label.y + e.label.h / 2 } : null
  let best: { a: Point; b: Point; score: number } | null = null
  for (const s of e.sections) {
    const pts = [s.start, ...s.bends, s.end]
    for (let i = 1; i < pts.length; i++) {
      const a = pts[i - 1]!
      const b = pts[i]!
      // with an ELK box: the segment closest to its centre (negative distance); without: the longest one
      const score = c ? -Math.hypot(c.x - Math.min(Math.max(c.x, Math.min(a.x, b.x)), Math.max(a.x, b.x)), c.y - Math.min(Math.max(c.y, Math.min(a.y, b.y)), Math.max(a.y, b.y))) : Math.hypot(b.x - a.x, b.y - a.y)
      if (!best || score > best.score) best = { a, b, score }
    }
  }
  if (!best) return { at: { x: 0, y: 0 }, along: 'horizontal', side: 'before' }
  const along = Math.abs(best.b.x - best.a.x) >= Math.abs(best.b.y - best.a.y) ? 'horizontal' : 'vertical'
  if (!c) return { at: pointAt([best.a, best.b], 0.5), along, side: 'before' }
  const lo = along === 'horizontal' ? Math.min(best.a.x, best.b.x) : Math.min(best.a.y, best.b.y)
  const hi = along === 'horizontal' ? Math.max(best.a.x, best.b.x) : Math.max(best.a.y, best.b.y)
  const t = Math.min(Math.max(along === 'horizontal' ? c.x : c.y, lo), hi)
  const at = along === 'horizontal' ? { x: t, y: best.a.y } : { x: best.a.x, y: t }
  const side = (along === 'horizontal' ? c.y > at.y : c.x > at.x) ? 'after' : 'before'
  return { at, along, side }
}

/**
 * One connector (§5.4 #1–#2): the ELK sections as a single r=8 path, dashed by `style`, coloured and headed by
 * `state`/`arrow`, its label on a `card` mask off the longest run; a transparent 18 px stroke is the hit area.
 */
export function EdgePath({ edge: e, state, style, arrow, markerId, tokens: t, label, full, labelFill }: { edge: Pick<LayoutEdge, 'sections' | 'label'>; state: UnitState; style?: 'solid' | 'dashed' | null; arrow?: Arrow | null; markerId: string | null; tokens: DiagramTokens; label?: string | null; full?: string; labelFill?: string }) {
  const d = sectionsPath(e.sections)
  const p = paint(state, t)
  const m = markers(arrow, markerFor(markerId, state))
  const l = label ? edgeLabelAt(e) : null
  return (
    <>
      <path d={d} fill="none" stroke="transparent" strokeWidth={18} />
      <path d={d} fill="none" stroke={p.stroke} strokeWidth={state === 'active' ? 2 : 1.25} strokeDasharray={style === 'dashed' ? '6 4' : undefined} {...m} />
      {l && label && <MaskedLabel text={label} full={full} at={l.at} along={l.along} side={l.side} tokens={t} fill={labelFill ?? p.stroke} />}
    </>
  )
}

/**
 * §5.3 #4 for the ELK family: ArrowRight/Down follow the first outgoing edge, ArrowLeft/Up the first incoming one;
 * without such an edge the kernel's authored-order step stands. `out(id)`/`inc(id)` name the neighbours.
 */
export function edgeArrows(attrs: UnitAttrs, id: string, out: (id: string) => string | undefined, inc: (id: string) => string | undefined): UnitAttrs {
  const base = attrs.onKeyDown
  return {
    ...attrs,
    onKeyDown: (e: KeyboardEvent) => {
      const forward = e.key === 'ArrowRight' || e.key === 'ArrowDown'
      const back = e.key === 'ArrowLeft' || e.key === 'ArrowUp'
      const next = forward ? out(id) : back ? inc(id) : undefined
      if (next !== undefined && next !== id) {
        e.preventDefault()
        focusUnit(e.currentTarget, next)
        return
      }
      base(e)
    },
  }
}

/** Unit id of edge `i` (edges are units too: they light with their nodes and carry the full label in the tooltip). */
export const edgeUnit = (i: number) => `edge:${i}`

/**
 * The unit contract for a node/edge graph: nodes then edges in authored order; a node lights its edges, an edge its
 * two nodes; arrows follow edges. Shared by Graph, StateMachine, Swimlane and GraphDelta.
 */
export function useGraphUnits(nodeIds: readonly string[], edges: readonly { from: string; to: string }[], highlight?: readonly string[], onSelect?: (id: string) => void): Units {
  const ids = useMemo(() => [...nodeIds, ...edges.map((_, i) => edgeUnit(i))], [nodeIds, edges])
  const incident = useCallback(
    (id: string): string[] => {
      if (id.startsWith('edge:')) {
        const e = edges[Number(id.slice(5))]
        return e ? [e.from, e.to] : []
      }
      const out: string[] = []
      edges.forEach((e, i) => (e.from === id || e.to === id) && out.push(edgeUnit(i)))
      return out
    },
    [edges],
  )
  const units = useUnits({ ids, incident, highlight, onSelect })
  const out = useCallback((id: string) => edges.find((e) => e.from === id && e.to !== id)?.to, [edges])
  const inc = useCallback((id: string) => edges.find((e) => e.to === id && e.from !== id)?.from, [edges])
  const unit = useCallback((id: string, label: string) => edgeArrows(units.unit(id, label), id, out, inc), [units, out, inc])
  return { ...units, unit }
}

/** The node's `<text>` block: label (600), `sub` mono under it, `tag` as a small rx=2 type tag; icon on the left. */
function NodeBody({ node, x, y, w, h, t, fill }: { node: Node; x: number; y: number; w: number; h: number; t: DiagramTokens; fill: string }) {
  const extra = (node.sub ? 1 : 0) + (node.tag ? 1 : 0)
  const lines = wrapLabel(node.label).length
  const total = lines + extra
  const top = y + h / 2 - ((total - 1) * LINE_PX) / 2
  const cx = x + w / 2 + (node.icon ? ICON_W / 2 : 0)
  const tagW = node.tag ? node.tag.length * 6.6 + 8 : 0
  return (
    <>
      {node.icon && <NodeIcon icon={node.icon} x={x + 10} y={y + h / 2 - 8} color={fill} />}
      <Label text={node.label} cx={cx} cy={top + ((lines - 1) * LINE_PX) / 2} fill={fill} t={t} weight={600} />
      {node.sub && (
        <text x={cx} y={top + lines * LINE_PX} textAnchor="middle" dominantBaseline="central" fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
          {node.sub}
        </text>
      )}
      {node.tag && (
        <g data-tag={node.tag}>
          <rect x={cx - tagW / 2} y={top + (lines + (node.sub ? 1 : 0)) * LINE_PX - 8} width={tagW} height={16} rx={2} fill={t.muted} />
          <text x={cx} y={top + (lines + (node.sub ? 1 : 0)) * LINE_PX} textAnchor="middle" dominantBaseline="central" fontFamily={MONO} fontSize={10} fill={t.mutedForeground}>
            {node.tag}
          </text>
        </g>
      )}
    </>
  )
}

export interface GraphBodyProps {
  graph: GraphData
  layout: Layout
  tokens: DiagramTokens
  markerId: string | null
  /** The unit contract (`GraphView`, GraphDelta); without it the legacy `highlight`/`onSelect` buttons apply (codemap). */
  units?: Units
  highlight?: readonly string[]
  onSelect?: (id: string) => void
  badges?: Record<string, string>
  hatch?: readonly string[]
  patternId?: string
  nodeClass?: string
}

/** The drawing in layout coordinates: groups → edges → nodes → labels (123 brief §5.4 #6). */
export function GraphBody({ graph, layout: l, tokens: t, markerId, units, highlight, onSelect, badges, hatch, patternId, nodeClass }: GraphBodyProps) {
  const hatchId = `${patternId ?? markerId ?? 'hatch'}-hatch`
  const nodes: Record<string, Node> = {}
  for (const n of graph.nodes) nodes[n.id] = n
  const groups: Record<string, GraphData['groups'] extends (infer G)[] | null | undefined ? G : never> = {}
  for (const g of graph.groups ?? []) groups[g.id] = g
  const edgeState = (e: Edge, i: number): UnitState => (units ? units.paint(edgeUnit(i), e.state) : (e.state ?? 'default'))
  const nodeState = (n: Node): UnitState => (units ? units.paint(n.id, n.state) : effectiveState(n, highlight))
  return (
    <>
      {l.groups.map((g) => {
        const grp = groups[g.id]
        return (
          <g key={g.id} data-group={g.id}>
            <rect x={g.x} y={g.y} width={g.w} height={g.h} rx={8} fill={t.muted} fillOpacity={0.35} stroke={t.border} strokeDasharray={grp?.kind === 'boundary' ? '6 4' : undefined} />
            <text x={g.x + 12} y={g.y + 16} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.mutedForeground}>
              {grp?.label ?? g.id}
            </text>
          </g>
        )
      })}
      {l.edges.map((e) => {
        const edge = graph.edges[e.index]
        if (!edge) return null
        const s = edgeState(edge, e.index)
        const label = [nodes[edge.from]?.label ?? edge.from, '→', nodes[edge.to]?.label ?? edge.to, edge.label ? `: ${edge.label}` : ''].join(' ')
        return (
          <g key={e.index} opacity={paint(s, t).opacity} data-unit={edgeUnit(e.index)} {...(units ? units.unit(edgeUnit(e.index), label) : {})}>
            <EdgePath edge={e} state={s} style={edge.style} arrow={edge.arrow} markerId={markerId} tokens={t} label={edge.label} />
          </g>
        )
      })}
      {l.nodes.map((n) => {
        const node = nodes[n.id]
        if (!node) return null
        const st = nodeState(node)
        const s = paint(st, t)
        const legacy = !units && onSelect ? { role: 'button' as const, tabIndex: 0, style: { cursor: 'pointer' }, onClick: () => onSelect(n.id), onKeyDown: (ev: KeyboardEvent) => (ev.key === 'Enter' || ev.key === ' ') && onSelect(n.id) } : {}
        return (
          <g key={n.id} className={nodeClass} opacity={s.opacity} data-node={n.id} data-unit={n.id} aria-description={node.ref ?? undefined} {...(units ? units.unit(n.id, unitLabel(node.label, node.sub, st)) : legacy)}>
            <rect x={n.x - 4} y={n.y - 4} width={n.w + 8} height={n.h + 8} fill="transparent" />
            <Shape kind={node.kind} x={n.x} y={n.y} w={n.w} h={n.h} fill={s.fill} fillOpacity={s.fillOpacity} stroke={s.stroke} strokeWidth={s.strokeWidth} />
            {hatch?.includes(n.id) && <rect x={n.x} y={n.y} width={n.w} height={n.h} rx={node.kind === 'pill' ? n.h / 2 : undefined} fill={`url(#${hatchId})`} />}
            <NodeBody node={node} x={n.x} y={n.y} w={n.w} h={n.h} t={t} fill={t.foreground} />
            {badges?.[n.id] && (
              <text x={n.x + n.w - 4} y={n.y - 3} textAnchor="end" fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
                {badges[n.id]}
              </text>
            )}
          </g>
        )
      })}
    </>
  )
}

/** Legend entries of a graph: a swatch in the state's colour, a line for an edge style, a swatch for a kind. */
export function graphLegend(g: GraphData, t: DiagramTokens): LegendItem[] {
  return (g.legend ?? []).map((e, i) => ({
    id: `legend:${i}`,
    label: e.label,
    color: e.state ? paint(e.state, t).stroke : t.foreground,
    shape: e.style ? 'line' : 'swatch',
    dashed: e.style === 'dashed',
  }))
}

/** Space above the drawing for a `title` (sans 600, 13 px). */
export const TITLE_H = 24
/** The margin around every ELK-family drawing. */
export const MARGIN = 12

/**
 * The svg for `graph` at `layout`; `markerId` must be unique per mounted svg (arrowheads are looked up by id) — `null`
 * draws no arrowheads (73 §6.5: none in Fit). `badges` are the colour states' words (73 §6.5), `hatch` the stale ids.
 */
export function GraphSvg({ graph, layout: l, tokens: t, markerId, units, highlight, onSelect, badges, hatch, patternId, nodeClass, root }: GraphBodyProps & { root?: { onKeyDown: (e: KeyboardEvent) => void } }) {
  const hatchId = `${patternId ?? markerId ?? 'hatch'}-hatch`
  const legend = graphLegend(graph, t)
  const top = MARGIN + (graph.title ? TITLE_H : 0)
  const W = Math.ceil(l.width + 2 * MARGIN)
  const H = Math.ceil(top + l.height + MARGIN + (legend.length ? LEGEND_H + 4 : 0))
  const a = svgA11y(markerId ?? patternId ?? 'graph', !!(onSelect || units))
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...root}>
      <title id={a.titleId}>{graph.title ?? 'Graph'}</title>
      <desc id={a.descId}>{describe('Graph', graph.title, graph.nodes.length, 'nodes')}</desc>
      <defs>
        {markerId !== null && <ArrowMarkers id={markerId} tokens={t} />}
        {hatch?.length ? (
          <pattern id={hatchId} width={6} height={6} patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
            <line x1={0} y1={0} x2={0} y2={6} stroke={t.mutedForeground} strokeWidth={2} opacity={0.3} />
          </pattern>
        ) : null}
      </defs>
      {graph.title && (
        <text x={MARGIN} y={MARGIN + 13} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
          {graph.title}
        </text>
      )}
      <g transform={`translate(${MARGIN} ${top})`}>
        <GraphBody graph={graph} layout={l} tokens={t} markerId={markerId} units={units} highlight={highlight} onSelect={onSelect} badges={badges} hatch={hatch} patternId={patternId} nodeClass={nodeClass} />
      </g>
      {legend.length > 0 && <Legend items={legend} x={MARGIN} y={H - LEGEND_H - 2} tokens={t} />}
    </svg>
  )
}

const GLYPH: Record<GraphState, string> = { active: '◉', done: '●', muted: '○', danger: '✗', default: '•' }

/** `crates/surface::graph::adjacency_lines` in TS: what the card shows when the layout times out. */
export function adjacencyText(g: GraphData): string {
  const label: Record<string, string> = {}
  for (const n of g.nodes) label[n.id] = n.label
  const line = (n: Node, indent: string) => {
    const targets = g.edges.filter((e) => e.from === n.id).map((e) => `${e.state === 'danger' ? '✗→ ' : ''}${e.label ? `${label[e.to] ?? e.to} (${e.label})` : (label[e.to] ?? e.to)}`)
    const extra = `${n.sub ? ` (${n.sub})` : ''}${n.tag ? ` [${n.tag}]` : ''}`
    return `${indent}${GLYPH[n.state ?? 'default']} ${n.label}${extra}${targets.length ? ` → ${targets.join(', ')}` : ''}`
  }
  const groups = g.groups ?? []
  const known: Record<string, true> = {}
  for (const grp of groups) known[grp.id] = true
  const out: string[] = []
  const block = (grp: (typeof groups)[number], depth: number) => {
    const pad = '  '.repeat(depth)
    out.push(`${pad}${grp.label}`)
    for (const n of g.nodes) if (n.group === grp.id) out.push(line(n, `${pad}  `))
    if (depth < 3) for (const c of groups) if (c.parent === grp.id) block(c, depth + 1)
  }
  for (const grp of groups) if (!(grp.parent && known[grp.parent])) block(grp, 0)
  for (const n of g.nodes) if (!(n.group && known[n.group])) out.push(line(n, ''))
  return out.join('\n')
}

export interface GraphViewProps {
  props: GraphData
  /** The fence is still streaming: edges naming absent nodes are dropped instead of drawn. */
  loading?: boolean
  /** Node ids drawn `active` (apps, 65 §3.5). */
  highlight?: readonly string[]
  /** A node was pressed (apps). */
  onSelect?: (id: string) => void
}

/** The tooltip of the hovered unit of a node/edge graph: label, `sub`, `tag`, state; an edge's label. */
export function graphTooltip(g: GraphData, hover: string | null): Tip | null {
  if (hover === null) return null
  if (hover.startsWith('edge:')) {
    const e = g.edges[Number(hover.slice(5))]
    if (!e) return null
    const name = (id: string) => g.nodes.find((n) => n.id === id)?.label ?? id
    return { title: `${name(e.from)} → ${name(e.to)}`, rows: [{ label: 'label', value: e.label }, { label: 'state', value: e.state }] }
  }
  const n = g.nodes.find((x) => x.id === hover)
  return n ? { title: n.label, rows: [{ label: 'sub', value: n.sub }, { label: 'tag', value: n.tag }, { label: 'state', value: n.state }, { label: 'ref', value: n.ref }] } : null
}

/** A tooltip's content: the hovered unit's title and rows. */
export interface Tip {
  title: string
  rows: { label: string; value?: string | number | null }[]
}

/**
 * The frame every ELK-family view sits in: `DiagramFrame` (pan/zoom, export, fullscreen) plus the one portal
 * tooltip for the hovered unit; `timeout` titles the frame when the §4.4 text stands in for the picture.
 */
export function ElkFrame({ kind, hash, timeout, hover, tip, children }: { kind: string; hash: string; timeout: boolean; hover: string | null; tip: Tip | null; children: ReactNode }) {
  const wrap = useRef<HTMLDivElement>(null)
  return (
    <div ref={wrap}>
      <DiagramFrame name={`${kind}-${hash}`} label={kind} kind={`${kind}-block`} title={timeout ? 'layout timed out; showing the text' : undefined}>
        {children}
      </DiagramFrame>
      {tip && <DiagramTooltip hover={hover} root={wrap.current} title={tip.title} rows={tip.rows} />}
    </div>
  )
}

/**
 * The Graph card: layout through the render queue (coalesced while streaming, last good layout stays on screen),
 * the svg in the shared `DiagramFrame`, the adjacency list when ELK exceeds 2 s — never a blank card.
 */
export function GraphView({ props, loading, highlight, onSelect }: GraphViewProps) {
  const graph = useMemo(() => (loading ? dropDangling(props) : props), [props, loading])
  const hash = useMemo(() => structuralHash(graph), [graph])
  const tokens = useDiagramTokens()
  const shown = useElkLayout(hash, () => toElk(graph), flattenElk)
  // The graph the shown layout belongs to: while a newer fence streams, the last good pair stays on screen.
  const drawnRef = useRef(graph)
  const drawn = shown && shown.hash === hash ? graph : drawnRef.current
  useEffect(() => {
    drawnRef.current = drawn
  }, [drawn])
  const nodeIds = useMemo(() => drawn.nodes.map((n) => n.id), [drawn])
  const units = useGraphUnits(nodeIds, drawn.edges, highlight, onSelect)
  let content: ReactNode
  if (shown && 'layout' in shown) {
    content = <GraphSvg graph={drawn} layout={shown.layout} tokens={tokens} markerId={`arrow-${shown.hash}`} units={units} root={units.root} />
  } else if (shown) {
    content = <pre className="p-3 font-mono text-xs leading-5 whitespace-pre-wrap">{adjacencyText(drawn)}</pre>
  }
  return (
    <ElkFrame kind="graph" hash={hash} timeout={!!shown && 'timeout' in shown} hover={units.state.hover} tip={graphTooltip(drawn, units.state.hover)}>
      {content}
    </ElkFrame>
  )
}
