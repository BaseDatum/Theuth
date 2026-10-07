// 123 §5.2(c) (W3): `Loop` — steps on a circle, clockwise from the top, the hub label centred. `LoopSvg` is the pure
// drawing over `loopLayout()` (snapshot-tested); `LoopView` wraps it in `DiagramFrame` with the theme tokens. The
// arrows are circular arcs: this is the one place curves are right (§5.4 #1 asks for orthogonal connectors
// everywhere else) — a cycle drawn with elbows reads as a flowchart, not a loop.
import { useId, useMemo, useRef, useSyncExternalStore } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { fnv } from '@/lib/diagram/elk'
import { truncate } from '@/lib/diagram/label'
import { DiagramTooltip } from '@/lib/diagram/tooltip'
import type { DiagramHostProps, UnitState } from '@/lib/diagram/types'
import { useUnits } from '@/lib/diagram/units'
import { type DiagramTokens, diagramTokens } from '@/lib/diagram-theme'
import { subscribeDark } from '@/lib/theme'
import { DiagramFrame } from '../../DiagramFrame'
import type { loopProps } from '../catalog'

export type LoopData = z.infer<typeof loopProps>

const M = 12
const BOX_W = 104
const BOX_H = 36
const FONT = 13

export interface LoopLayout {
  cx: number
  cy: number
  r: number
  /** Box centres, index = step index. */
  steps: { x: number; y: number; w: number; h: number }[]
  /** Arc from step `i` to step `(i + 1) % n`. */
  arcs: { from: number; to: number; d: string; mid: { x: number; y: number } }[]
  width: number
  height: number
}

/** Steps on a circle of radius `r` (grows with `n`), arcs trimmed so they start and end just outside the boxes. */
export function loopLayout(n: number, top = 0): LoopLayout {
  const r = Math.max(110, n * 28)
  const cx = M + BOX_W / 2 + r
  const cy = top + M + BOX_H / 2 + r
  const at = (a: number) => ({ x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) })
  const angle = (i: number) => -Math.PI / 2 + (i * 2 * Math.PI) / Math.max(1, n)
  const gap = Math.asin(Math.min(0.45, (BOX_W / 2 + 4) / r))
  const steps = Array.from({ length: n }, (_, i) => ({ ...at(angle(i)), w: BOX_W, h: BOX_H }))
  const arcs = Array.from({ length: n }, (_, i) => {
    const a0 = angle(i) + gap
    const a1 = angle(i + 1) - gap
    const p0 = at(a0)
    const p1 = at(a1)
    const large = a1 - a0 > Math.PI ? 1 : 0
    return { from: i, to: (i + 1) % n, d: `M${p0.x.toFixed(1)} ${p0.y.toFixed(1)} A${r} ${r} 0 ${large} 1 ${p1.x.toFixed(1)} ${p1.y.toFixed(1)}`, mid: at((a0 + a1) / 2) }
  })
  return { cx, cy, r, steps, arcs, width: Math.ceil(2 * cx), height: Math.ceil(cy + r + BOX_H / 2 + M) }
}

function tint(state: UnitState, t: DiagramTokens): { stroke: string; fill: string; fillOpacity: number; opacity: number; width: number } {
  switch (state) {
    case 'active':
      return { stroke: t.accent, fill: t.accent, fillOpacity: 0.1, opacity: 1, width: 2 }
    case 'done':
      return { stroke: t.success, fill: t.success, fillOpacity: 0.12, opacity: 1, width: 1.25 }
    case 'danger':
      return { stroke: t.danger, fill: t.danger, fillOpacity: 0.12, opacity: 1, width: 1.5 }
    case 'muted':
      return { stroke: t.mutedForeground, fill: t.card, fillOpacity: 1, opacity: 0.5, width: 1 }
    default:
      return { stroke: t.foreground, fill: t.card, fillOpacity: 1, opacity: 1, width: 1 }
  }
}

/** The `<desc>`: the first line of the Rust lowering (`render.rs` Loop arm). */
export function loopDesc(d: LoopData): string {
  return describe('Loop', d.title, d.steps.length, 'steps')
}

export function LoopSvg({ data: d, layout: l, tokens: t, markerId, highlight, onSelect }: { data: LoopData; layout: LoopLayout; tokens: DiagramTokens; markerId: string; highlight?: readonly string[]; onSelect?: (id: string) => void }) {
  const ids = useMemo(() => d.steps.map((_, i) => String(i)), [d.steps])
  const incident = useMemo(() => (id: string) => [String((Number(id) + 1) % ids.length), String((Number(id) + ids.length - 1) % ids.length)], [ids.length])
  const u = useUnits({ ids, incident, highlight, onSelect })
  const a = svgA11y(markerId, !!onSelect)
  const ref = useRef<SVGSVGElement>(null)
  const hover = u.state.hover === null ? null : d.steps[Number(u.state.hover)]
  return (
    <>
      <svg ref={ref} xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${l.width} ${l.height}`} width={l.width} height={l.height} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...u.root}>
        <title id={a.titleId}>{d.title ?? 'Loop'}</title>
        <desc id={a.descId}>{loopDesc(d)}</desc>
        <defs>
          <marker id={markerId} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto">
            <path d="M0 0 L10 5 L0 10 z" fill={t.foreground} />
          </marker>
        </defs>
        {d.title && (
          <text x={M} y={18} fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
            {d.title}
          </text>
        )}
        {l.arcs.slice(0, d.steps.length).map((arc) => (
          <path key={arc.from} d={arc.d} fill="none" stroke={t.foreground} strokeWidth={1.25} markerEnd={`url(#${markerId})`} opacity={u.paint(String(arc.from)) === 'muted' && u.paint(String(arc.to)) === 'muted' ? 0.5 : 1} />
        ))}
        {d.hub && (
          <text x={l.cx} y={l.cy} textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.mutedForeground}>
            {truncate(d.hub, 20)}
          </text>
        )}
        {d.steps.map((s, i) => {
          const b = l.steps[i]
          if (!b) return null
          const p = tint(u.paint(String(i), s.state), t)
          return (
            <g key={i} {...u.unit(String(i), unitLabel(`${i + 1}. ${s.label}`, s.note, s.state))} aria-description={s.ref ?? undefined} opacity={p.opacity}>
              <rect x={b.x - b.w / 2 - 4} y={b.y - b.h / 2 - 4} width={b.w + 8} height={b.h + 8} fill="transparent" />
              <rect x={b.x - b.w / 2} y={b.y - b.h / 2} width={b.w} height={b.h} rx={8} fill={p.fill} fillOpacity={p.fillOpacity} stroke={p.stroke} strokeWidth={p.width} />
              <text x={b.x} y={b.y} textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
                {truncate(s.label, 14)}
              </text>
            </g>
          )
        })}
      </svg>
      <DiagramTooltip hover={u.state.hover} root={ref.current} title={hover?.label ?? ''} rows={[{ label: 'note', value: hover?.note }, { label: 'state', value: hover?.state }, { label: 'ref', value: hover?.ref }]} />
    </>
  )
}

export function LoopView({ props, highlight, onSelect }: DiagramHostProps<LoopData>) {
  const id = useId().replace(/:/g, '')
  const hash = useMemo(() => fnv(JSON.stringify(props)), [props])
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'), () => false)
  const tokens = useMemo(diagramTokens, [dark])
  const layout = useMemo(() => loopLayout(props.steps.length, props.title ? 28 : 0), [props.steps.length, props.title])
  return (
    <DiagramFrame name={`loop-${hash}`} label="loop" kind="loop-block">
      <LoopSvg data={props} layout={layout} tokens={tokens} markerId={`loop-${id}`} highlight={highlight} onSelect={onSelect} />
    </DiagramFrame>
  )
}
