// 123 §5.2(c) (W3): `Fishbone` — the effect box at the right of a horizontal spine, one 60° bone per cause
// alternating above/below (first cause nearest the head), sub-causes as horizontal ticks off the bone. `FishboneSvg`
// is the pure drawing over `fishboneLayout()` (snapshot-tested); `FishboneView` wraps it in `DiagramFrame`.
import { useId, useMemo, useRef, useSyncExternalStore } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { fnv } from '@/lib/diagram/elk'
import { truncate } from '@/lib/diagram/label'
import { DiagramTooltip } from '@/lib/diagram/tooltip'
import type { DiagramHostProps } from '@/lib/diagram/types'
import { useUnits } from '@/lib/diagram/units'
import { type DiagramTokens, diagramTokens } from '@/lib/diagram-theme'
import { subscribeDark } from '@/lib/theme'
import { DiagramFrame } from '../../DiagramFrame'
import type { fishboneProps } from '../catalog'

export type FishboneData = z.infer<typeof fishboneProps>

const M = 12
const FONT = 13
const MONO_PX = 11
const MONO = 'ui-monospace, monospace'
const PAIR_GAP = 160
const HEAD_GAP = 80
const EFFECT_H = 40
const TICK = 24
const TICK_STEP = 24
const TICK_START = 28
const MAX_ITEMS = 6
const ITEM_CHARS = 20
const LABEL_CHARS = 16
/** 60° bones: horizontal run per unit of height. */
const LEAN = 1 / Math.tan(Math.PI / 3)
export const EFFECT_ID = 'effect'

export interface FishboneBone {
  /** +1 above the spine, −1 below. */
  side: 1 | -1
  foot: { x: number; y: number }
  end: { x: number; y: number }
  /** Category label mask centred just beyond the outer end. */
  label: { x: number; y: number; w: number; h: number }
  /** Horizontal ticks off the bone (≤ 6), text to the left of the tick. */
  ticks: { from: { x: number; y: number }; to: { x: number; y: number }; text: string; full: string }[]
}

export interface FishboneLayout {
  spineY: number
  spine: { x0: number; x1: number }
  effect: { x: number; y: number; w: number; h: number }
  /** Index = cause index. */
  bones: FishboneBone[]
  width: number
  height: number
}

export function fishboneLayout(d: FishboneData, top = 0): FishboneLayout {
  const n = d.causes.length
  const pairs = Math.max(1, Math.ceil(n / 2))
  const heights = d.causes.map((c) => 40 + 24 * Math.min(MAX_ITEMS, c.items.length))
  const maxH = Math.max(40, ...heights)
  const spineY = top + 32 + maxH
  const effectW = Math.max(96, Math.min(200, 8 + d.effect.length * 8))
  // lay out with the effect's left edge at 0, then shift so the leftmost text sits at the margin
  const effectX = 0
  let minX = -(HEAD_GAP + PAIR_GAP * (pairs - 1))
  const bones = d.causes.map((c, i): FishboneBone => {
    const side: 1 | -1 = i % 2 === 0 ? 1 : -1
    const h = heights[i] ?? 40
    const fx = effectX - HEAD_GAP - PAIR_GAP * Math.floor(i / 2)
    const end = { x: fx - h * LEAN, y: spineY - side * h }
    const shown = truncate(c.category, LABEL_CHARS)
    // ponytail: no DOM in layout — label width is `chars × 13 × 0.6 + 8`.
    const lw = Math.ceil(shown.length * FONT * 0.6 + 8)
    const label = { x: end.x - lw / 2, y: end.y - (side === 1 ? 20 : 0), w: lw, h: 20 }
    const ticks = c.items.slice(0, MAX_ITEMS).map((item, k) => {
      const dist = TICK_START + TICK_STEP * k
      const from = { x: fx - dist * LEAN, y: spineY - side * dist }
      const text = truncate(item, ITEM_CHARS)
      minX = Math.min(minX, from.x - TICK - 4 - text.length * MONO_PX * 0.6)
      return { from, to: { x: from.x - TICK, y: from.y }, text, full: item }
    })
    minX = Math.min(minX, label.x)
    return { side, foot: { x: fx, y: spineY }, end, label, ticks }
  })
  const dx = Math.ceil(M - minX)
  for (const b of bones) {
    b.foot.x += dx
    b.end.x += dx
    b.label.x += dx
    for (const t of b.ticks) {
      t.from.x += dx
      t.to.x += dx
    }
  }
  const effect = { x: effectX + dx, y: spineY - EFFECT_H / 2, w: effectW, h: EFFECT_H }
  return { spineY, spine: { x0: M, x1: effect.x - 2 }, effect, bones, width: Math.ceil(effect.x + effectW + M), height: Math.ceil(top + 2 * maxH + 64) }
}

/** The `<desc>`: the first line of the Rust lowering (`render.rs` Fishbone arm). */
export function fishboneDesc(d: FishboneData): string {
  return describe('Fishbone', d.title, d.causes.length, 'causes')
}

export function FishboneSvg({ data: d, layout: l, tokens: t, markerId, highlight, onSelect }: { data: FishboneData; layout: FishboneLayout; tokens: DiagramTokens; markerId: string; highlight?: readonly string[]; onSelect?: (id: string) => void }) {
  const ids = useMemo(() => [...d.causes.map((_, i) => String(i)), EFFECT_ID], [d.causes])
  const u = useUnits({ ids, highlight, onSelect })
  const a = svgA11y(markerId, !!onSelect)
  const ref = useRef<SVGSVGElement>(null)
  const hover = u.state.hover === null || u.state.hover === EFFECT_ID ? null : d.causes[Number(u.state.hover)]
  const effectPaint = u.paint(EFFECT_ID)
  return (
    <>
      <svg ref={ref} xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${l.width} ${l.height}`} width={l.width} height={l.height} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...u.root}>
        <title id={a.titleId}>{d.title ?? 'Fishbone'}</title>
        <desc id={a.descId}>{fishboneDesc(d)}</desc>
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
        <line x1={l.spine.x0} y1={l.spineY} x2={l.spine.x1} y2={l.spineY} stroke={t.foreground} strokeWidth={1.5} markerEnd={`url(#${markerId})`} />
        {d.causes.map((c, i) => {
          const b = l.bones[i]
          if (!b) return null
          const p = u.paint(String(i))
          const active = p === 'active'
          return (
            <g key={i} {...u.unit(String(i), unitLabel(c.category, c.items.join(', ')))} opacity={p === 'muted' ? 0.5 : 1}>
              <line x1={b.foot.x} y1={b.foot.y} x2={b.end.x} y2={b.end.y} stroke="transparent" strokeWidth={18} />
              <line x1={b.foot.x} y1={b.foot.y} x2={b.end.x} y2={b.end.y} stroke={active ? t.accent : t.foreground} strokeWidth={active ? 2 : 1.25} />
              {b.ticks.map((k, j) => (
                <g key={j}>
                  <title>{k.full}</title>
                  <line x1={k.from.x} y1={k.from.y} x2={k.to.x} y2={k.to.y} stroke={t.mutedForeground} strokeWidth={1} />
                  <text x={k.to.x - 4} y={k.to.y} textAnchor="end" dominantBaseline="central" fontFamily={MONO} fontSize={MONO_PX} fill={t.foreground}>
                    {k.text}
                  </text>
                </g>
              ))}
              <rect x={b.label.x} y={b.label.y} width={b.label.w} height={b.label.h} rx={3} fill={t.card} />
              <text x={b.label.x + b.label.w / 2} y={b.label.y + b.label.h / 2} textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={active ? t.accent : t.foreground}>
                {truncate(c.category, LABEL_CHARS)}
              </text>
            </g>
          )
        })}
        <g {...u.unit(EFFECT_ID, d.effect)} opacity={effectPaint === 'muted' ? 0.5 : 1}>
          <rect x={l.effect.x} y={l.effect.y} width={l.effect.w} height={l.effect.h} rx={6} fill={effectPaint === 'active' ? t.accent : t.card} fillOpacity={effectPaint === 'active' ? 0.1 : 1} stroke={effectPaint === 'active' ? t.accent : t.border} strokeWidth={effectPaint === 'active' ? 2 : 1} />
          <text x={l.effect.x + l.effect.w / 2} y={l.spineY} textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
            {truncate(d.effect, 24)}
          </text>
        </g>
      </svg>
      <DiagramTooltip hover={u.state.hover} root={ref.current} title={u.state.hover === EFFECT_ID ? d.effect : (hover?.category ?? '')} rows={hover ? [{ label: 'items', value: hover.items.join(', ') }] : []} />
    </>
  )
}

export function FishboneView({ props, highlight, onSelect }: DiagramHostProps<FishboneData>) {
  const id = useId().replace(/:/g, '')
  const hash = useMemo(() => fnv(JSON.stringify(props)), [props])
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'), () => false)
  const tokens = useMemo(diagramTokens, [dark])
  const layout = useMemo(() => fishboneLayout(props, props.title ? 28 : 0), [props])
  return (
    <DiagramFrame name={`fishbone-${hash}`} label="fishbone" kind="fishbone-block">
      <FishboneSvg data={props} layout={layout} tokens={tokens} markerId={`fishbone-${id}`} highlight={highlight} onSelect={onSelect} />
    </DiagramFrame>
  )
}
