// Design 123 §4.2, §5.2(a) (W1): `Swimlane` — ELK layered with partitioning (`org.eclipse.elk.partitioning.activate`
// on the root, `.partition` = lane index on every step, verified in elkjs 0.9.3's elk-worker), so a lane is one band
// across the layout axis; bands are drawn from the layer extents with the lane label at the head. `data` steps are
// parallelograms, `decision` diamonds, `start|end` pills, `step` rounded boxes.
import type { ElkNode } from 'elkjs/lib/elk-api.js'
import { type ReactNode, useEffect, useMemo, useRef } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { ELK_OPTIONS, fnv, labelSize, nodeSize, norm } from '@/lib/diagram/elk'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import type { Units } from '@/lib/diagram/units'
import type { DiagramTokens } from '@/lib/diagram-theme'
import { EDGE_OPTIONS, flattenElk, FONT_PX, type Layout, LINE_PX, useDiagramTokens, useElkLayout } from '@/lib/graph-layout'
import { ArrowMarkers, EdgePath, ElkFrame, Label, MARGIN, MONO, edgeUnit, paint, Shape, type Tip, TITLE_H, useGraphUnits } from '../Graph'
import type { swimlaneProps } from '../catalog'

export type SwimlaneData = z.infer<typeof swimlaneProps>
type Step = SwimlaneData['steps'][number]

/** The lane-label head: a strip above the bands (LR) or left of them (TB). */
export const HEAD = 28
/** Band padding around a lane's extent, also the gap between bands' content and their border. */
const BAND_PAD = 16

/** Steps in a known lane; edges whose ends both exist (streaming tolerance). */
export function dropDangling(p: SwimlaneData): SwimlaneData {
  const lanes: Record<string, true> = {}
  for (const l of p.lanes) lanes[l.id] = true
  const steps = p.steps.filter((s) => lanes[s.lane])
  const ids: Record<string, true> = {}
  for (const s of steps) ids[s.id] = true
  return { ...p, steps, edges: p.edges.filter((e) => ids[e.from] && ids[e.to]) }
}

export function structuralHash(p: SwimlaneData): string {
  return fnv(
    [
      `sw:${p.direction ?? 'LR'}:${norm(p.title)}`,
      ...p.lanes.map((l) => `l\u0001${l.id}\u0001${norm(l.label)}`),
      ...p.steps.map((s) => `s\u0001${s.id}\u0001${s.lane}\u0001${s.kind ?? ''}\u0001${norm(s.label)}\u0001${norm(s.sub)}\u0001${s.state ?? ''}`),
      ...p.edges.map((e) => `e\u0001${e.from}\u0001${e.to}\u0001${norm(e.label)}\u0001${e.style ?? ''}`),
    ].join('\u0002'),
  )
}

/** Steps as partitioned nodes (`partition` = lane index); edges on the root with their label sizes. */
export function toElk(p: SwimlaneData): ElkNode {
  const laneIndex: Record<string, number> = {}
  p.lanes.forEach((l, i) => (laneIndex[l.id] = i))
  return {
    id: 'root',
    layoutOptions: { ...ELK_OPTIONS, ...EDGE_OPTIONS, 'elk.direction': p.direction === 'TB' ? 'DOWN' : 'RIGHT', 'org.eclipse.elk.partitioning.activate': 'true' },
    children: p.steps.map((s) => {
      const size = nodeSize(s.label, s.sub ? 1 : 0)
      const wide = s.kind === 'decision' || s.kind === 'data' ? 20 : 0
      return { id: `n:${s.id}`, width: size.w + wide, height: size.h, layoutOptions: { 'org.eclipse.elk.partitioning.partition': String(laneIndex[s.lane] ?? 0) } }
    }),
    edges: p.edges.map((e, i) => {
      const l = labelSize(e.label)
      return { id: `e:${i}`, sources: [`n:${e.from}`], targets: [`n:${e.to}`], labels: l ? [{ id: `l:${i}`, text: norm(e.label), width: l.w, height: l.h }] : [] }
    }),
  }
}

export interface Band {
  id: string
  label: string
  /** Along the layout axis: `x`/`w` for LR, `y`/`h` for TB (in layout coordinates). */
  from: number
  to: number
}

/**
 * Lane bands from the layer extents: each lane spans its steps' boxes ± `BAND_PAD`; neighbouring bands meet at the
 * midpoint of the gap so the bands tile the drawing. An empty lane gets a minimal band after the previous one.
 */
export function bands(p: SwimlaneData, l: Layout): Band[] {
  const horizontal = p.direction !== 'TB'
  const at: Record<string, { x: number; y: number; w: number; h: number }> = {}
  for (const n of l.nodes) at[n.id] = n
  const raw = p.lanes.map((lane) => {
    let lo = Number.POSITIVE_INFINITY
    let hi = Number.NEGATIVE_INFINITY
    for (const s of p.steps) {
      const b = s.lane === lane.id ? at[s.id] : undefined
      if (!b) continue
      lo = Math.min(lo, horizontal ? b.x : b.y)
      hi = Math.max(hi, horizontal ? b.x + b.w : b.y + b.h)
    }
    return { id: lane.id, label: lane.label, lo, hi }
  })
  const out: Band[] = []
  let cursor = -BAND_PAD
  for (let i = 0; i < raw.length; i++) {
    const r = raw[i]!
    const next = raw.slice(i + 1).find((x) => Number.isFinite(x.lo))
    const from = i === 0 ? -BAND_PAD : cursor
    let to: number
    if (Number.isFinite(r.hi)) to = next ? (r.hi + next.lo) / 2 : r.hi + BAND_PAD
    else to = from + 2 * BAND_PAD
    out.push({ id: r.id, label: r.label, from, to })
    cursor = to
  }
  const end = (horizontal ? l.width : l.height) + BAND_PAD
  if (out.length > 0 && out[out.length - 1]!.to < end) out[out.length - 1]!.to = end
  return out
}

function stepKind(s: Step): 'rounded' | 'diamond' | 'pill' | 'parallelogram' {
  switch (s.kind) {
    case 'decision':
      return 'diamond'
    case 'start':
    case 'end':
      return 'pill'
    case 'data':
      return 'parallelogram'
    default:
      return 'rounded'
  }
}

/** The TS twin of the Rust lowering (lane headers, steps, cross-lane edges), for the time-out. */
export function swimlaneText(p: SwimlaneData): string {
  const lane: Record<string, string> = {}
  const name: Record<string, string> = {}
  for (const s of p.steps) {
    lane[s.id] = s.lane
    name[s.id] = s.label
  }
  // the Rust `swimlane_lines` glyphs verbatim (the time-out twin must equal the w100 golden)
  const mark = (s: Step) => ({ decision: '◇', start: '▶', end: '■', data: '▱', step: '' })[s.kind ?? 'step'] || { active: '◉', done: '●', muted: '○', danger: '✗', default: '•' }[s.state ?? 'default'] // no-emoji: allow
  const out: string[] = []
  if (p.title) out.push(p.title)
  for (const l of p.lanes) {
    out.push(l.label)
    for (const s of p.steps) {
      if (s.lane !== l.id) continue
      const same = p.edges.filter((e) => e.from === s.id && lane[e.to] === l.id).map((e) => `${name[e.to] ?? e.to}${e.label ? ` (${e.label})` : ''}`)
      out.push(`  ${mark(s)} ${s.label}${s.sub ? ` (${s.sub})` : ''}${same.length ? ` → ${same.join(', ')}` : ''}`)
    }
  }
  const cross = p.edges.filter((e) => lane[e.from] !== lane[e.to]).map((e) => `${name[e.from] ?? e.from} ${e.style === 'dashed' ? '⇢' : '→'} ${name[e.to] ?? e.to}${e.label ? `  ${e.label}` : ''}`)
  if (cross.length) out.push('', ...cross)
  return out.join('\n')
}

export function swimlaneTooltip(p: SwimlaneData, hover: string | null): Tip | null {
  if (hover === null) return null
  if (hover.startsWith('edge:')) {
    const e = p.edges[Number(hover.slice(5))]
    if (!e) return null
    const name = (id: string) => p.steps.find((s) => s.id === id)?.label ?? id
    return { title: `${name(e.from)} → ${name(e.to)}`, rows: [{ label: 'label', value: e.label }] }
  }
  const s = p.steps.find((x) => x.id === hover)
  return s ? { title: s.label, rows: [{ label: 'lane', value: p.lanes.find((l) => l.id === s.lane)?.label }, { label: 'sub', value: s.sub }, { label: 'kind', value: s.kind }, { label: 'state', value: s.state }, { label: 'ref', value: s.ref }] } : null
}

/** The svg at `layout`. Draw order: bands → edges → steps → labels. */
export function SwimlaneSvg({ props: p, layout: l, tokens: t, markerId, units, root }: { props: SwimlaneData; layout: Layout; tokens: DiagramTokens; markerId: string; units?: Units; root?: { onKeyDown: (e: React.KeyboardEvent) => void } }) {
  const horizontal = p.direction !== 'TB'
  const b = bands(p, l)
  const headW = horizontal ? 0 : Math.max(64, ...p.lanes.map((x) => x.label.length * 7 + 16))
  const left = MARGIN + BAND_PAD + headW
  const top = MARGIN + (p.title ? TITLE_H : 0) + (horizontal ? HEAD : 0) + BAND_PAD
  const W = Math.ceil(left + l.width + BAND_PAD + MARGIN)
  const H = Math.ceil(top + l.height + BAND_PAD + MARGIN)
  const a = svgA11y(markerId, !!units)
  const steps: Record<string, Step> = {}
  for (const s of p.steps) steps[s.id] = s
  const name = (id: string) => steps[id]?.label ?? id
  const bandTop = top - BAND_PAD - (horizontal ? HEAD : 0)
  const bandH = l.height + 2 * BAND_PAD + (horizontal ? HEAD : 0)
  const bandLeft = left - BAND_PAD - headW
  const bandW = l.width + 2 * BAND_PAD + headW
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${W} ${H}`} width={W} height={H} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...root}>
      <title id={a.titleId}>{p.title ?? 'Swimlane'}</title>
      <desc id={a.descId}>{describe('Swimlane', p.title, p.steps.length, 'steps')}</desc>
      <defs>
        <ArrowMarkers id={markerId} tokens={t} />
      </defs>
      {p.title && (
        <text x={MARGIN} y={MARGIN + 13} fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.foreground}>
          {p.title}
        </text>
      )}
      {b.map((band, i) =>
        horizontal ? (
          <g key={band.id} data-lane={band.id}>
            <rect x={left + band.from} y={bandTop} width={band.to - band.from} height={bandH} fill={i % 2 ? t.muted : t.card} fillOpacity={i % 2 ? 0.25 : 1} stroke={t.border} />
            <text x={left + (band.from + band.to) / 2} y={bandTop + HEAD / 2} textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.mutedForeground}>
              {band.label}
            </text>
          </g>
        ) : (
          <g key={band.id} data-lane={band.id}>
            <rect x={bandLeft} y={top + band.from} width={bandW} height={band.to - band.from} fill={i % 2 ? t.muted : t.card} fillOpacity={i % 2 ? 0.25 : 1} stroke={t.border} />
            <text x={bandLeft + 8} y={top + (band.from + band.to) / 2} dominantBaseline="central" fontFamily={t.font} fontSize={FONT_PX} fontWeight={600} fill={t.mutedForeground}>
              {band.label}
            </text>
          </g>
        ),
      )}
      <g transform={`translate(${left} ${top})`}>
        {l.edges.map((e) => {
          const edge = p.edges[e.index]
          if (!edge) return null
          const st: UnitState = units ? units.paint(edgeUnit(e.index), null) : 'default'
          return (
            <g key={e.index} opacity={paint(st, t).opacity} data-unit={edgeUnit(e.index)} {...(units ? units.unit(edgeUnit(e.index), `${name(edge.from)} → ${name(edge.to)}${edge.label ? `: ${edge.label}` : ''}`) : {})}>
              <EdgePath edge={e} state={st} style={edge.style} markerId={markerId} tokens={t} label={edge.label} />
            </g>
          )
        })}
        {l.nodes.map((n) => {
          const s = steps[n.id]
          if (!s) return null
          const st = units ? units.paint(n.id, s.state) : (s.state ?? 'default')
          const pt = paint(st, t)
          return (
            <g key={n.id} opacity={pt.opacity} data-unit={n.id} data-kind={s.kind ?? 'step'} aria-description={s.ref ?? undefined} {...(units ? units.unit(n.id, unitLabel(s.label, s.sub, st)) : {})}>
              <rect x={n.x - 4} y={n.y - 4} width={n.w + 8} height={n.h + 8} fill="transparent" />
              <Shape kind={stepKind(s)} x={n.x} y={n.y} w={n.w} h={n.h} fill={pt.fill} fillOpacity={pt.fillOpacity} stroke={pt.stroke} strokeWidth={pt.strokeWidth} />
              <Label text={s.label} cx={n.x + n.w / 2} cy={n.y + n.h / 2 - (s.sub ? LINE_PX / 2 : 0)} fill={t.foreground} t={t} weight={600} />
              {s.sub && (
                <text x={n.x + n.w / 2} y={n.y + n.h / 2 + LINE_PX / 2} textAnchor="middle" dominantBaseline="central" fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
                  {s.sub}
                </text>
              )}
            </g>
          )
        })}
      </g>
    </svg>
  )
}

export function SwimlaneView({ props, loading, highlight, onSelect }: DiagramHostProps<SwimlaneData>) {
  const p = useMemo(() => (loading ? dropDangling(props) : props), [props, loading])
  const hash = useMemo(() => structuralHash(p), [p])
  const tokens = useDiagramTokens()
  const shown = useElkLayout(hash, () => toElk(p), flattenElk)
  const drawnRef = useRef(p)
  const drawn = shown && shown.hash === hash ? p : drawnRef.current
  useEffect(() => {
    drawnRef.current = drawn
  }, [drawn])
  const ids = useMemo(() => drawn.steps.map((s) => s.id), [drawn])
  const units = useGraphUnits(ids, drawn.edges, highlight, onSelect)
  let content: ReactNode
  if (shown && 'layout' in shown) content = <SwimlaneSvg props={drawn} layout={shown.layout} tokens={tokens} markerId={`sw-${shown.hash}`} units={units} root={units.root} />
  else if (shown) content = <pre className="p-3 font-mono text-xs leading-5 whitespace-pre-wrap">{swimlaneText(drawn)}</pre>
  return (
    <ElkFrame kind="swimlane" hash={hash} timeout={!!shown && 'timeout' in shown} hover={units.state.hover} tip={swimlaneTooltip(drawn, units.state.hover)}>
      {content}
    </ElkFrame>
  )
}
