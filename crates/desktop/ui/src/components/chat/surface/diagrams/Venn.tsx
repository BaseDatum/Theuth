// 123 §5.2(c) (W3): `Venn` — two or three equal circles at fixed geometry, overlap labels at the lens centroids,
// set labels outside their circle. `VennSvg` is the pure drawing over `vennLayout()` (snapshot-tested); `VennView`
// wraps it in `DiagramFrame` with the theme tokens.
import { useId, useMemo, useRef, useSyncExternalStore } from 'react'
import type { z } from 'zod'
import { describe, svgA11y, unitLabel } from '@/lib/diagram/a11y'
import { fnv } from '@/lib/diagram/elk'
import { truncate } from '@/lib/diagram/label'
import { DiagramTooltip } from '@/lib/diagram/tooltip'
import type { DiagramHostProps, Point } from '@/lib/diagram/types'
import { useUnits } from '@/lib/diagram/units'
import { type DiagramTokens, diagramTokens } from '@/lib/diagram-theme'
import { subscribeDark } from '@/lib/theme'
import { DiagramFrame } from '../../DiagramFrame'
import type { vennProps } from '../catalog'

export type VennData = z.infer<typeof vennProps>

const M = 12
const W = 400
const R = 96
const SIDE = 112
const LABEL_H = 40
const FONT = 13
const MONO = 'ui-monospace, monospace'

export interface VennLayout {
  n: 2 | 3
  r: number
  /** Circle centres, index = set index in authored order. */
  circles: Point[]
  /** Lens centroid of each 2-set overlap (`sets` are circle indices, ascending). */
  lens: { sets: [number, number]; at: Point }[]
  /** The 3-set region centroid (= the circle centres' centroid). */
  center: Point
  /** Set label anchor points (first line baseline) and text anchors. */
  labels: { at: Point; anchor: 'start' | 'middle' | 'end' }[]
  width: number
  height: number
}

export function vennLayout(n: 2 | 3, top = 0): VennLayout {
  const cx = M + W / 2
  if (n === 2) {
    const cy = top + M + R
    const circles = [
      { x: cx - 56, y: cy },
      { x: cx + 56, y: cy },
    ]
    // ponytail: left/right of the circles is only 48 px wide, so the 2-set labels sit under the outer halves
    const ly = cy + R + 20
    return {
      n,
      r: R,
      circles,
      lens: [{ sets: [0, 1], at: { x: cx, y: cy } }],
      center: { x: cx, y: cy },
      labels: [
        { at: { x: cx - 56 - R / 2, y: ly }, anchor: 'middle' },
        { at: { x: cx + 56 + R / 2, y: ly }, anchor: 'middle' },
      ],
      width: W + 2 * M,
      height: top + 2 * M + 2 * R + LABEL_H,
    }
  }
  const rc = SIDE / Math.sqrt(3) // circumradius of the centre triangle
  const cy = top + M + LABEL_H + rc + R
  const circles = [
    { x: cx, y: cy - rc },
    { x: cx - SIDE / 2, y: cy + rc / 2 },
    { x: cx + SIDE / 2, y: cy + rc / 2 },
  ]
  const center = { x: cx, y: cy }
  const lens: VennLayout['lens'] = []
  for (let i = 0; i < 3; i++) {
    for (let j = i + 1; j < 3; j++) {
      const a = circles[i]!
      const b = circles[j]!
      const c = circles[3 - i - j]!
      const mx = (a.x + b.x) / 2
      const my = (a.y + b.y) / 2
      const dx = mx - c.x
      const dy = my - c.y
      const len = Math.hypot(dx, dy) || 1
      lens.push({ sets: [i, j], at: { x: mx + (28 * dx) / len, y: my + (28 * dy) / len } })
    }
  }
  const below = cy + rc / 2 + R + 16
  return {
    n,
    r: R,
    circles,
    lens,
    center,
    labels: [
      { at: { x: cx, y: cy - rc - R - 20 }, anchor: 'middle' },
      { at: { x: cx - SIDE / 2 - 24, y: below }, anchor: 'middle' },
      { at: { x: cx + SIDE / 2 + 24, y: below }, anchor: 'middle' },
    ],
    width: W + 2 * M,
    height: Math.ceil((top + 2 * M + 2 * LABEL_H + rc + rc / 2 + 2 * R) / 4) * 4,
  }
}

/** The region point for a set-id list; `null` for unknown ids, duplicates, or a 3-set region on a 2-set layout. */
export function vennRegion(sets: readonly string[], layout: VennLayout, ids: readonly string[]): Point | null {
  const idx = sets.map((s) => ids.indexOf(s))
  if (idx.some((i) => i < 0 || i >= layout.n) || new Set(idx).size !== idx.length) return null
  if (idx.length === 3) return layout.n === 3 ? layout.center : null
  if (idx.length !== 2) return null
  const [a, b] = [Math.min(idx[0]!, idx[1]!), Math.max(idx[0]!, idx[1]!)]
  return layout.lens.find((l) => l.sets[0] === a && l.sets[1] === b)?.at ?? null
}

/** The `<desc>`: the first line of the Rust lowering (`render.rs` Venn arm). */
export function vennDesc(d: VennData): string {
  return describe('Venn', d.title, d.sets.length, 'sets')
}

export function VennSvg({ data: d, layout: l, tokens: t, markerId, highlight, onSelect }: { data: VennData; layout: VennLayout; tokens: DiagramTokens; markerId: string; highlight?: readonly string[]; onSelect?: (id: string) => void }) {
  const sets = useMemo(() => (d.sets.length < 2 ? [] : d.sets.slice(0, l.n)), [d.sets, l.n])
  const setIds = useMemo(() => sets.map((s) => s.id), [sets])
  // dangling (streaming) overlaps are dropped; keep the region point beside each survivor
  const overlaps = useMemo(
    () =>
      (d.overlaps ?? []).flatMap((o) => {
        const at = vennRegion(o.sets, l, setIds)
        return at ? [{ ...o, id: o.sets.join('∩'), at }] : []
      }),
    [d.overlaps, l, setIds],
  )
  const ids = useMemo(() => [...setIds, ...overlaps.map((o) => o.id)], [setIds, overlaps])
  const incident = useMemo(
    () => (id: string) => {
      const o = overlaps.find((x) => x.id === id)
      return o ? o.sets : overlaps.filter((x) => x.sets.includes(id)).map((x) => x.id)
    },
    [overlaps],
  )
  const u = useUnits({ ids, incident, highlight, onSelect })
  const a = svgA11y(markerId, !!onSelect)
  const ref = useRef<SVGSVGElement>(null)
  const hs = sets.find((s) => s.id === u.state.hover)
  const ho = hs ? undefined : overlaps.find((o) => o.id === u.state.hover)
  return (
    <>
      <svg ref={ref} xmlns="http://www.w3.org/2000/svg" viewBox={`0 0 ${l.width} ${l.height}`} width={l.width} height={l.height} style={{ maxWidth: '100%', height: 'auto' }} {...a.svg} {...u.root}>
        <title id={a.titleId}>{d.title ?? 'Venn'}</title>
        <desc id={a.descId}>{vennDesc(d)}</desc>
        {d.title && (
          <text x={M} y={18} fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
            {d.title}
          </text>
        )}
        {sets.map((s, i) => {
          const c = l.circles[i]
          const lb = l.labels[i]
          if (!c || !lb) return null
          const state = u.paint(s.id)
          const color = t.series[i % 6] ?? t.foreground
          return (
            <g key={s.id} {...u.unit(s.id, unitLabel(s.label, s.size != null ? String(s.size) : undefined))} opacity={state === 'muted' ? 0.5 : 1}>
              <circle cx={c.x} cy={c.y} r={l.r} fill={color} fillOpacity={0.18} stroke={state === 'active' ? t.accent : color} strokeWidth={state === 'active' ? 2 : 1.25} />
              <text x={lb.at.x} y={lb.at.y} textAnchor={lb.anchor} fontFamily={t.font} fontSize={FONT} fontWeight={600} fill={t.foreground}>
                {truncate(s.label, 18)}
              </text>
              {s.size != null && (
                <text x={lb.at.x} y={lb.at.y + 14} textAnchor={lb.anchor} fontFamily={MONO} fontSize={11} fill={t.mutedForeground}>
                  {s.size}
                </text>
              )}
            </g>
          )
        })}
        {overlaps.map((o) => {
          const state = u.paint(o.id)
          return (
            <g key={o.id} {...u.unit(o.id, o.label)} opacity={state === 'muted' ? 0.5 : 1}>
              <circle cx={o.at.x} cy={o.at.y} r={18} fill="transparent" />
              <text x={o.at.x} y={o.at.y} textAnchor="middle" dominantBaseline="central" fontFamily={t.font} fontSize={12} fontWeight={state === 'active' ? 600 : 400} fill={state === 'active' ? t.accent : t.foreground}>
                {truncate(o.label, 18)}
              </text>
            </g>
          )
        })}
      </svg>
      <DiagramTooltip
        hover={u.state.hover}
        root={ref.current}
        title={hs?.label ?? ho?.label ?? ''}
        rows={[
          { label: 'size', value: hs?.size != null ? String(hs.size) : undefined },
          { label: 'sets', value: ho ? ho.sets.map((id) => sets.find((s) => s.id === id)?.label ?? id).join(' ∩ ') : undefined },
        ]}
      />
    </>
  )
}

export function VennView({ props, highlight, onSelect }: DiagramHostProps<VennData>) {
  const id = useId().replace(/:/g, '')
  const hash = useMemo(() => fnv(JSON.stringify(props)), [props])
  const dark = useSyncExternalStore(subscribeDark, () => document.documentElement.classList.contains('dark'), () => false)
  const tokens = useMemo(diagramTokens, [dark])
  const n = props.sets.length >= 3 ? 3 : 2
  const layout = useMemo(() => vennLayout(n, props.title ? 28 : 0), [n, props.title])
  return (
    <DiagramFrame name={`venn-${hash}`} label="venn" kind="venn-block">
      <VennSvg data={props} layout={layout} tokens={tokens} markerId={`venn-${id}`} highlight={highlight} onSelect={onSelect} />
    </DiagramFrame>
  )
}
